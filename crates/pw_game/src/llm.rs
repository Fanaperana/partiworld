//! A bridge to a language model that can take over a person's mind.
//!
//! By default the game runs its own small model (Qwen3 0.6B, ONNX, on the
//! CPU; see `pw_brain`). It can instead use any OpenAI-style chat endpoint
//! (Ollama, llama.cpp's server, LM Studio, or a hosted API). Either way the
//! work happens on a background thread, so the simulation never waits on the
//! model. Replies come back as JSON:
//! `{"thought": "...", "say": "...", "do": "..."}`; anything the model gets
//! wrong is forgiven (plain text becomes speech, unknown actions are ignored).

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmConfig {
    /// Run the game's own model (in `model_dir`) instead of calling `url`.
    pub builtin: bool,
    /// Folder with the built-in model's ONNX file and tokenizer.
    pub model_dir: String,
    /// Built-in model: most tokens spent reasoning before deciding (0 = no reasoning).
    pub think_tokens: u32,
    /// Built-in model: CPU threads to use (0 = half the cores).
    pub threads: u32,
    /// Everyone (colonists and villagers) thinks with the model unless unticked one by one.
    pub everyone: bool,
    /// Chat completions URL.
    pub url: String,
    pub model: String,
    /// Sent as a bearer token if not empty (hosted APIs).
    pub api_key: String,
    /// In-game seconds (at 1x) between a hooked person's decisions.
    pub think_every: f32,
    pub max_tokens: u32,
    pub temperature: f32,
    /// Ask the server for JSON output (dropped automatically if it refuses).
    pub json_mode: bool,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            builtin: true,
            model_dir: pw_brain::DEFAULT_DIR.into(),
            think_tokens: 256,
            threads: 0,
            everyone: true,
            url: "http://localhost:11434/v1/chat/completions".into(),
            model: "llama3.2:3b".into(),
            api_key: String::new(),
            think_every: 20.0,
            max_tokens: 160,
            temperature: 0.8,
            json_mode: true,
        }
    }
}

impl LlmConfig {
    /// The model's name for messages.
    pub fn label(&self) -> String {
        if self.builtin { "the built-in Qwen3 0.6B".into() } else { self.model.clone() }
    }

    /// Is the built-in model downloaded?
    pub fn builtin_ready(&self) -> bool {
        pw_brain::model_file(Path::new(&self.model_dir)).is_some()
    }

    /// Can a model be asked at all (a server, or the built-in one downloaded)?
    pub fn usable(&self) -> bool {
        !self.builtin || self.builtin_ready()
    }

    /// From a JSON file, then `PW_LLM_URL` / `PW_LLM_MODEL` / `PW_LLM_KEY`.
    pub fn load(path: &Path) -> Self {
        let mut c: LlmConfig = std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        if let Ok(v) = std::env::var("PW_LLM_URL") {
            c.url = v;
        }
        if let Ok(v) = std::env::var("PW_LLM_MODEL") {
            c.model = v;
        }
        if let Ok(v) = std::env::var("PW_LLM_KEY") {
            c.api_key = v;
        }
        c
    }
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let s = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, s).map_err(|e| e.to_string())
    }
}

/// The answer to `find_models`: model names, or why there are none.
pub type FoundModels = Result<Vec<String>, String>;

/// What a request is for.
#[derive(Clone, Debug, PartialEq)]
pub enum Purpose {
    /// Decide what to do next (and maybe say something).
    Decide,
    /// The next line in conversation `convo`.
    Chat { convo: u64 },
    /// The settings window's "Test" button.
    Test,
}

struct Request {
    uid: u64,
    purpose: Purpose,
    system: String,
    user: String,
    cfg: LlmConfig,
}

/// A parsed reply.
#[derive(Clone, Debug, Default)]
pub struct Reply {
    pub uid: u64,
    pub purpose: Option<Purpose>,
    pub thought: String,
    pub say: String,
    pub act: String,
    /// Raw text, for the log.
    pub raw: String,
    pub error: Option<String>,
    pub secs: f32,
}

pub struct Llm {
    pub cfg: LlmConfig,
    tx: Option<Sender<Request>>,
    rx: Option<Mutex<Receiver<Reply>>>,
    /// People waiting on an answer.
    pub pending: HashSet<u64>,
    /// Last status line for the UI.
    pub status: String,
    pub calls: u32,
    pub errors: u32,
    /// Models the server says it has, once asked (`find_models`).
    pub found: Arc<Mutex<Option<FoundModels>>>,
}

/// Most requests in flight at once; local models answer one at a time.
const MAX_PENDING: usize = 6;

impl Default for Llm {
    fn default() -> Self {
        Self::new(LlmConfig::default())
    }
}

impl Llm {
    pub fn new(cfg: LlmConfig) -> Self {
        Self { cfg, tx: None, rx: None, pending: HashSet::new(), status: "Not connected yet.".into(), calls: 0, errors: 0, found: Arc::new(Mutex::new(None)) }
    }

    fn start(&mut self) {
        let (tx, worker_rx) = channel::<Request>();
        let (worker_tx, rx) = channel::<Reply>();
        let spawned = std::thread::Builder::new().name("pw-llm".into()).spawn(move || {
            // The built-in model, loaded on first use and kept.
            let mut local: Option<(String, pw_brain::LocalModel)> = None;
            let mut seed = 0x9E37_79B9_7F4A_7C15u64;
            while let Ok(req) = worker_rx.recv() {
                let t0 = Instant::now();
                let answer = if req.cfg.builtin {
                    seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                    run_builtin(&mut local, &req, seed)
                } else {
                    call(&req.cfg, &req.system, &req.user).map(|t| (t, String::new()))
                };
                let mut reply = match answer {
                    Ok((text, thinking)) => {
                        let mut r = parse_reply(&text);
                        // No thought in the answer: use the end of its reasoning, if readable.
                        if r.thought.is_empty() && !thinking.is_empty() && thinking.is_ascii() {
                            let last = thinking.trim().rsplit(['.', '\n']).find(|s| s.trim().len() > 12).unwrap_or("").trim();
                            r.thought = clip(last, 200);
                        }
                        r
                    }
                    Err(e) => Reply { error: Some(e), ..Default::default() },
                };
                reply.uid = req.uid;
                reply.purpose = Some(req.purpose);
                reply.secs = t0.elapsed().as_secs_f32();
                if worker_tx.send(reply).is_err() {
                    break;
                }
            }
        });
        if spawned.is_ok() {
            self.tx = Some(tx);
            self.rx = Some(Mutex::new(rx));
        }
    }

    /// Ask the server which models it has, in the background.
    pub fn find_models(&self) {
        let slot = self.found.clone();
        let cfg = self.cfg.clone();
        if let Ok(mut s) = slot.lock() {
            *s = None;
        }
        let _ = std::thread::Builder::new().name("pw-llm-models".into()).spawn(move || {
            let r = list_models(&cfg);
            if let Ok(mut s) = slot.lock() {
                *s = Some(r);
            }
        });
    }

    pub fn busy(&self, uid: u64) -> bool {
        self.pending.contains(&uid)
    }

    /// Queue a request. False if too many are waiting already.
    pub fn ask(&mut self, uid: u64, purpose: Purpose, system: String, user: String) -> bool {
        self.ask_with(uid, purpose, system, user, true)
    }

    /// Room for one more request?
    pub fn has_room(&self) -> bool {
        self.pending.len() < MAX_PENDING
    }

    /// Queue a request; `reason` false skips the built-in model's reasoning (faster).
    pub fn ask_with(&mut self, uid: u64, purpose: Purpose, system: String, user: String, reason: bool) -> bool {
        if self.pending.len() >= MAX_PENDING || self.pending.contains(&uid) {
            return false;
        }
        if self.tx.is_none() {
            self.start();
        }
        let Some(tx) = &self.tx else { return false };
        let mut cfg = self.cfg.clone();
        if !reason {
            cfg.think_tokens = 0;
        }
        let req = Request { uid, purpose, system, user, cfg };
        if tx.send(req).is_err() {
            self.tx = None;
            return false;
        }
        self.pending.insert(uid);
        true
    }

    /// Answers that have come in since the last call.
    pub fn poll(&mut self) -> Vec<Reply> {
        let mut out = Vec::new();
        if let Some(rx) = &self.rx
            && let Ok(rx) = rx.lock()
        {
            while let Ok(r) = rx.try_recv() {
                out.push(r);
            }
        }
        for r in &out {
            self.pending.remove(&r.uid);
            self.calls += 1;
            match &r.error {
                Some(e) => {
                    self.errors += 1;
                    self.status = format!("Error: {e}");
                }
                None => self.status = format!("OK - {} answered in {:.1}s", self.cfg.label(), r.secs),
            }
        }
        out
    }
}

/// Answer with the game's own model, loading it first if needed.
/// Returns (answer, reasoning).
fn run_builtin(local: &mut Option<(String, pw_brain::LocalModel)>, req: &Request, seed: u64) -> Result<(String, String), String> {
    let cfg = &req.cfg;
    if local.as_ref().is_none_or(|(dir, _)| dir != &cfg.model_dir) {
        let threads = if cfg.threads > 0 {
            cfg.threads as usize
        } else {
            std::thread::available_parallelism().map(|n| n.get() / 2).unwrap_or(2).max(1)
        };
        let m = pw_brain::LocalModel::load(Path::new(&cfg.model_dir), threads).map_err(|e| format!("built-in model: {e}"))?;
        *local = Some((cfg.model_dir.clone(), m));
    }
    let (_, m) = local.as_mut().ok_or("built-in model not loaded")?;
    // Reason when deciding what to do; chat lines must come quickly.
    let think = if req.purpose == Purpose::Decide { cfg.think_tokens as usize } else { 0 };
    let opt = pw_brain::GenOptions { max_tokens: think + cfg.max_tokens as usize, think_tokens: think, temperature: cfg.temperature, seed };
    let g = m.chat(&req.system, &req.user, &opt)?;
    if g.text.trim().is_empty() {
        return Err("the model ran out of room while thinking (raise Max reply or lower Thinking)".into());
    }
    Ok((g.text, g.thinking))
}

fn agent(secs: u64) -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(secs))).http_status_as_error(false).build().into()
}

fn call(cfg: &LlmConfig, system: &str, user: &str) -> Result<String, String> {
    match call_once(cfg, system, user, cfg.json_mode) {
        // Some servers don't know JSON mode; ask again without it.
        Err(e) if cfg.json_mode && e.starts_with("HTTP 4") => call_once(cfg, system, user, false),
        r => r,
    }
}

fn call_once(cfg: &LlmConfig, system: &str, user: &str, json: bool) -> Result<String, String> {
    let mut body = serde_json::json!({
        "model": cfg.model,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user},
        ],
        "temperature": cfg.temperature,
        "max_tokens": cfg.max_tokens,
        "stream": false,
    });
    if json {
        body["response_format"] = serde_json::json!({"type": "json_object"});
    }
    let mut req = agent(90).post(&cfg.url).header("Content-Type", "application/json");
    if !cfg.api_key.is_empty() {
        req = req.header("Authorization", &format!("Bearer {}", cfg.api_key));
    }
    let mut resp = req.send(body.to_string()).map_err(|e| format!("can't reach {} ({e})", cfg.url))?;
    let status = resp.status().as_u16();
    let text = resp.body_mut().read_to_string().map_err(|e| e.to_string())?;
    if status >= 400 {
        let short: String = text.chars().take(200).collect();
        return Err(format!("HTTP {status}: {short}"));
    }
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("not JSON: {e}"))?;
    // OpenAI style, then Ollama's own /api/chat and /api/generate.
    let content = v["choices"][0]["message"]["content"].as_str().or_else(|| v["message"]["content"].as_str()).or_else(|| v["response"].as_str());
    let reasoning = v["choices"][0]["message"]["reasoning"].as_str().or_else(|| v["choices"][0]["message"]["reasoning_content"].as_str());
    match content {
        // Reasoning models can spend the whole budget thinking.
        Some(c) if c.trim().is_empty() && reasoning.is_some_and(|r| !r.is_empty()) => {
            Err("the model used up Max reply while thinking; raise it".into())
        }
        Some(c) => Ok(c.to_string()),
        None => Err("no message in the answer".into()),
    }
}

/// The models an OpenAI-style server offers (`/v1/models`), or Ollama's `/api/tags`.
fn list_models(cfg: &LlmConfig) -> Result<Vec<String>, String> {
    let base = match cfg.url.find("/v1/") {
        Some(i) => cfg.url[..i].to_string(),
        None => cfg.url.trim_end_matches('/').rsplit_once('/').map(|x| x.0.to_string()).unwrap_or_default(),
    };
    let mut last = String::new();
    for path in ["/v1/models", "/api/tags"] {
        let url = format!("{base}{path}");
        let mut req = agent(8).get(&url);
        if !cfg.api_key.is_empty() {
            req = req.header("Authorization", &format!("Bearer {}", cfg.api_key));
        }
        let mut resp = match req.call() {
            Ok(r) => r,
            Err(e) => {
                last = format!("can't reach {url} ({e})");
                continue;
            }
        };
        let text = resp.body_mut().read_to_string().unwrap_or_default();
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            last = format!("{url} didn't answer with JSON");
            continue;
        };
        let names: Vec<String> = v["data"]
            .as_array()
            .map(|a| a.iter().filter_map(|m| m["id"].as_str().map(String::from)).collect())
            .or_else(|| v["models"].as_array().map(|a| a.iter().filter_map(|m| m["name"].as_str().map(String::from)).collect()))
            .unwrap_or_default();
        if !names.is_empty() {
            return Ok(names);
        }
        last = format!("{url} lists no models");
    }
    Err(last)
}

/// Read a model's answer, forgiving what small models get wrong.
pub fn parse_reply(text: &str) -> Reply {
    // Reasoning models wrap their thinking in <think> tags first.
    let mut t = text.to_string();
    while let (Some(a), Some(b)) = (t.find("<think>"), t.find("</think>")) {
        if b < a {
            break;
        }
        t.replace_range(a..b + "</think>".len(), "");
    }
    let t = t.trim().trim_start_matches("```json").trim_start_matches("```").trim_end_matches("```").trim();
    let mut r = Reply { raw: t.to_string(), ..Default::default() };
    let field = |v: &serde_json::Value, k: &[&str]| k.iter().find_map(|k| v[*k].as_str().map(|s| s.trim().to_string())).unwrap_or_default();
    if let Some(v) = first_json(t) {
        r.thought = field(&v, &["thought", "think", "thinking"]);
        r.say = field(&v, &["say", "speech", "says", "line", "dialogue"]);
        // Small models put the action under different keys, sometimes next to junk.
        r.act = ["do", "action", "act"].iter().map(|k| field(&v, &[k])).find(|a| is_action(a)).unwrap_or_default();
    } else if t.contains('{') {
        // Broken JSON: dig the fields out by hand, and never speak the braces.
        r.thought = loose_field(t, &["thought", "think"]);
        r.say = loose_field(t, &["say", "speech"]);
        r.act = ["do", "action"].iter().map(|k| loose_field(t, &[k])).find(|a| is_action(a)).unwrap_or_default();
    } else {
        // Plain text: the first line is what they say.
        r.say = t.lines().next().unwrap_or("").to_string();
    }
    r.say = clip(&r.say, 140);
    r.thought = clip(&r.thought, 200);
    if r.say.contains('{') || r.say.contains("\":") {
        r.say.clear();
    }
    r
}

/// The first `{...}` that parses, trying each closing brace in turn.
fn first_json(t: &str) -> Option<serde_json::Value> {
    let start = t.find('{')?;
    t[start..].match_indices('}').find_map(|(i, _)| serde_json::from_str::<serde_json::Value>(&t[start..=start + i]).ok().filter(|v| v.is_object()))
}

/// `"key": "value"` from text that isn't valid JSON.
fn loose_field(t: &str, keys: &[&str]) -> String {
    for k in keys {
        let pat = format!("\"{k}\"");
        let Some(i) = t.find(&pat) else { continue };
        let rest = t[i + pat.len()..].trim_start().trim_start_matches(':').trim_start();
        if let Some(rest) = rest.strip_prefix('"') {
            // Cut-off answers just end; take what's there.
            let end = rest.find('"').unwrap_or(rest.len());
            return rest[..end].trim_end_matches(['}', '\n']).to_string();
        }
    }
    String::new()
}

/// Something `apply_act` understands.
pub fn is_action(a: &str) -> bool {
    let a = a.trim().to_lowercase().replace(['-', ' '], "_");
    let verb = a.split(':').next().unwrap_or("");
    ["work", "eat", "sleep", "warm_up", "warm", "warmup", "rest", "wait", "idle", "wander", "walk", "explore", "go_home", "home"].contains(&verb)
        || verb.starts_with("talk")
        || ["tell", "ask", "order"].contains(&verb)
}

fn clip(s: &str, n: usize) -> String {
    let s = s.trim().trim_matches('"').trim();
    if s.chars().count() <= n { s.to_string() } else { format!("{}...", s.chars().take(n).collect::<String>()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_json_inside_chatter() {
        let r = parse_reply("<think>hmm</think>Sure! {\"thought\": \"I'm hungry\", \"say\": \"Any berries left?\", \"do\": \"eat\"} ok");
        assert_eq!(r.thought, "I'm hungry");
        assert_eq!(r.say, "Any berries left?");
        assert_eq!(r.act, "eat");
    }

    #[test]
    fn broken_json_is_mended() {
        let r = parse_reply(r#"{"action": "talk to Rune", "name": "Rune", "say": "hello", "do": "social action"}}"#);
        assert_eq!(r.say, "hello");
        assert_eq!(r.act, "talk to Rune");
        let r = parse_reply(r#"{"thought": "hm", "say": "Ugo?"#);
        assert_eq!(r.say, "Ugo?");
        assert!(r.act.is_empty());
        let r = parse_reply(r#"{"think": "x", "do": "walk", "name": {"#);
        assert!(!r.say.contains('{'));
        assert_eq!(r.act, "walk");
    }

    #[test]
    fn plain_text_becomes_speech() {
        let r = parse_reply("\"Nice weather today.\"\nmore");
        assert_eq!(r.say, "Nice weather today.");
        assert!(r.act.is_empty());
    }
}
