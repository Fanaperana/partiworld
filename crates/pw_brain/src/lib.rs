//! A small language model that runs inside the game, on the CPU.
//!
//! Loads a decoder-only chat model exported to ONNX (made for Qwen3 0.6B:
//! `model_int8.onnx` + `tokenizer.json` + `config.json` from
//! onnx-community/Qwen3-0.6B-ONNX) and generates replies token by token,
//! keeping the attention cache between steps so each new token is cheap.
//!
//! Qwen3 thinks before it answers (`<think>...</think>`). Thinking is capped
//! at `think_tokens`: when the cap is hit the think block is closed for it, so
//! a reply always comes out in reasonable time on a slow CPU.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::time::Instant;

use ort::session::{Session, SessionInputValue, builder::GraphOptimizationLevel};
use ort::value::{DynValue, Tensor};
use tokenizers::Tokenizer;

/// Where the game looks for the model, relative to the game folder.
pub const DEFAULT_DIR: &str = "models/qwen3-0.6b";
/// Model file names tried in order (int8 runs best on CPUs).
const MODEL_FILES: [&str; 4] = ["model_int8.onnx", "model_quantized.onnx", "model_q4.onnx", "model.onnx"];

/// How to generate one reply.
#[derive(Clone, Debug)]
pub struct GenOptions {
    pub max_tokens: usize,
    /// Most tokens spent thinking before the answer is forced (0 = don't think).
    pub think_tokens: usize,
    pub temperature: f32,
    pub seed: u64,
}

impl Default for GenOptions {
    fn default() -> Self {
        Self { max_tokens: 700, think_tokens: 320, temperature: 0.7, seed: 1 }
    }
}

/// A finished reply.
#[derive(Clone, Debug, Default)]
pub struct Generated {
    /// The answer, thinking removed.
    pub text: String,
    /// What it thought first.
    pub thinking: String,
    pub tokens: usize,
    pub secs: f32,
}

pub struct LocalModel {
    session: Session,
    tok: Tokenizer,
    /// Cache inputs (`past_key_values.N.key`) and their matching outputs (`present.N.key`).
    cache: Vec<(String, String)>,
    kv_heads: i64,
    head_dim: i64,
    has_positions: bool,
    stop: Vec<u32>,
    think_open: Option<u32>,
    think_close: Option<u32>,
    pub name: String,
}

/// Is a usable model in `dir`?
pub fn model_file(dir: &Path) -> Option<PathBuf> {
    if !dir.join("tokenizer.json").exists() {
        return None;
    }
    MODEL_FILES.iter().map(|f| dir.join(f)).find(|p| p.exists())
}

impl LocalModel {
    /// Load the model in `dir`, using up to `threads` CPU threads.
    pub fn load(dir: &Path, threads: usize) -> Result<Self, String> {
        let file = model_file(dir).ok_or_else(|| format!("no model in {} (need tokenizer.json and {})", dir.display(), MODEL_FILES[0]))?;
        let tok = Tokenizer::from_file(dir.join("tokenizer.json")).map_err(|e| format!("tokenizer: {e}"))?;
        let config: serde_json::Value =
            std::fs::read_to_string(dir.join("config.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let env = ort::init().with_name("partiworld").build().map_err(|e| format!("onnx runtime: {e}"))?;
        let session = Session::builder(&env)
            .map_err(|e| e.to_string())?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| e.to_string())?
            .with_intra_threads(threads.max(1))
            .map_err(|e| e.to_string())?
            .commit_from_file(&file)
            .map_err(|e| format!("loading {}: {e}", file.display()))?;
        let inputs: Vec<String> = session.inputs().iter().map(|i| i.name().to_string()).collect();
        let outputs: Vec<String> = session.outputs().iter().map(|o| o.name().to_string()).collect();
        let mut cache = Vec::new();
        for i in inputs.iter().filter(|n| n.starts_with("past_key_values")) {
            let o = i.replacen("past_key_values", "present", 1);
            if !outputs.contains(&o) {
                return Err(format!("model has {i} but no {o}"));
            }
            cache.push((i.clone(), o));
        }
        if !outputs.iter().any(|o| o == "logits") {
            return Err("model has no logits output".into());
        }
        let num = |k: &str| config[k].as_i64();
        let heads = num("num_attention_heads").unwrap_or(16);
        let kv_heads = num("num_key_value_heads").unwrap_or(heads);
        let head_dim = num("head_dim").unwrap_or_else(|| num("hidden_size").unwrap_or(1024) / heads);
        let id = |s: &str| tok.token_to_id(s);
        let stop = ["<|im_end|>", "<|endoftext|>"].iter().filter_map(|s| id(s)).collect();
        let name = config["_name_or_path"].as_str().map(String::from).unwrap_or_else(|| dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
        Ok(Self {
            has_positions: inputs.iter().any(|n| n == "position_ids"),
            session,
            cache,
            kv_heads,
            head_dim,
            stop,
            think_open: id("<think>"),
            think_close: id("</think>"),
            tok,
            name,
        })
    }

    /// Answer one chat turn.
    pub fn chat(&mut self, system: &str, user: &str, opt: &GenOptions) -> Result<Generated, String> {
        let start = Instant::now();
        let think = opt.think_tokens > 0 && self.think_open.is_some();
        // Qwen's chat template; with thinking off, an empty think block is given.
        let mut prompt = format!("<|im_start|>system\n{system}<|im_end|>\n<|im_start|>user\n{user}<|im_end|>\n<|im_start|>assistant\n");
        if !think {
            prompt.push_str("<think>\n\n</think>\n\n");
        }
        let enc = self.tok.encode(prompt, false).map_err(|e| e.to_string())?;
        let mut feed: Vec<u32> = enc.get_ids().to_vec();
        let mut past: Vec<DynValue> = Vec::with_capacity(self.cache.len());
        for _ in &self.cache {
            let t = Tensor::<f32>::from_array((vec![1i64, self.kv_heads, 0, self.head_dim], Vec::<f32>::new())).map_err(|e| e.to_string())?;
            past.push(t.into_dyn());
        }
        let mut seen = 0usize; // tokens already in the cache
        let mut out: Vec<u32> = Vec::new();
        let mut rng = opt.seed.max(1);
        let mut thinking = think;
        let mut thought_for = 0usize;
        while out.len() < opt.max_tokens {
            let n = feed.len();
            let mut inputs: Vec<(Cow<str>, SessionInputValue)> = Vec::with_capacity(3 + past.len());
            let ids: Vec<i64> = feed.iter().map(|&t| t as i64).collect();
            inputs.push(("input_ids".into(), Tensor::from_array((vec![1i64, n as i64], ids)).map_err(|e| e.to_string())?.into()));
            let mask = vec![1i64; seen + n];
            inputs.push(("attention_mask".into(), Tensor::from_array((vec![1i64, (seen + n) as i64], mask)).map_err(|e| e.to_string())?.into()));
            if self.has_positions {
                let pos: Vec<i64> = (seen..seen + n).map(|p| p as i64).collect();
                inputs.push(("position_ids".into(), Tensor::from_array((vec![1i64, n as i64], pos)).map_err(|e| e.to_string())?.into()));
            }
            for ((name, _), v) in self.cache.iter().zip(past.drain(..)) {
                inputs.push((name.clone().into(), v.into()));
            }
            let mut res = self.session.run(inputs).map_err(|e| format!("running the model: {e}"))?;
            let next = {
                let (shape, logits) = res["logits"].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
                let vocab = *shape.last().unwrap_or(&0) as usize;
                let last = &logits[logits.len() - vocab..];
                sample(last, opt.temperature, &mut rng)
            };
            for (_, o) in &self.cache {
                past.push(res.remove(o.as_str()).ok_or_else(|| format!("no {o} in the output"))?);
            }
            drop(res);
            seen += n;
            if self.stop.contains(&next) {
                break;
            }
            out.push(next);
            feed = vec![next];
            if thinking {
                if Some(next) == self.think_close {
                    thinking = false;
                } else {
                    thought_for += 1;
                    // Out of thinking time: close the thought and answer.
                    if thought_for >= opt.think_tokens {
                        let close = self.tok.encode("\n</think>\n\n", false).map_err(|e| e.to_string())?;
                        out.extend_from_slice(close.get_ids());
                        feed = close.get_ids().to_vec();
                        thinking = false;
                    }
                }
            }
        }
        let text = self.tok.decode(&out, false).map_err(|e| e.to_string())?;
        let (thinking, text) = split_thinking(&text);
        Ok(Generated { text, thinking, tokens: out.len(), secs: start.elapsed().as_secs_f32() })
    }
}

/// Split `<think>...</think>answer` into (thinking, answer).
pub fn split_thinking(t: &str) -> (String, String) {
    let t = t.trim_start();
    let body = t.strip_prefix("<think>").unwrap_or(t);
    match body.find("</think>") {
        Some(i) => (body[..i].trim().to_string(), body[i + "</think>".len()..].trim().to_string()),
        None if t.starts_with("<think>") => (body.trim().to_string(), String::new()),
        None => (String::new(), t.trim().to_string()),
    }
}

/// Pick the next token: top-k 20, nucleus 0.95, at `temperature` (0 = greedy).
fn sample(logits: &[f32], temperature: f32, rng: &mut u64) -> u32 {
    if temperature <= 0.01 {
        return logits.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map(|(i, _)| i as u32).unwrap_or(0);
    }
    let mut top: Vec<(usize, f32)> = logits.iter().copied().enumerate().collect();
    let k = 20.min(top.len());
    top.select_nth_unstable_by(k - 1, |a, b| b.1.total_cmp(&a.1));
    top.truncate(k);
    top.sort_by(|a, b| b.1.total_cmp(&a.1));
    let max = top[0].1;
    let mut probs: Vec<f32> = top.iter().map(|(_, l)| ((l - max) / temperature).exp()).collect();
    let sum: f32 = probs.iter().sum();
    let mut acc = 0.0;
    let mut cut = probs.len();
    for (i, p) in probs.iter_mut().enumerate() {
        *p /= sum;
        acc += *p;
        if acc >= 0.95 {
            cut = i + 1;
            break;
        }
    }
    let total: f32 = probs[..cut].iter().sum();
    // xorshift64*
    *rng ^= *rng >> 12;
    *rng ^= *rng << 25;
    *rng ^= *rng >> 27;
    let r = ((rng.wrapping_mul(0x2545F4914F6CDD1D) >> 40) as f32 / (1u64 << 24) as f32) * total;
    let mut acc = 0.0;
    for i in 0..cut {
        acc += probs[i];
        if r <= acc {
            return top[i].0 as u32;
        }
    }
    top[cut - 1].0 as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_thinking_from_the_answer() {
        assert_eq!(split_thinking("<think>\nhm, food\n</think>\n\n{\"do\":\"eat\"}"), ("hm, food".into(), "{\"do\":\"eat\"}".into()));
        assert_eq!(split_thinking("plain"), (String::new(), "plain".into()));
        assert_eq!(split_thinking("<think>cut off"), ("cut off".into(), String::new()));
    }

    #[test]
    fn sampling_follows_the_logits() {
        let mut rng = 7;
        assert_eq!(sample(&[0.0, 5.0, 1.0], 0.0, &mut rng), 1);
        let picks: Vec<u32> = (0..200).map(|_| sample(&[0.0, 9.0, 0.0], 0.7, &mut rng)).collect();
        assert!(picks.iter().all(|&p| p == 1));
    }

    /// Needs the model files: `cargo test --release -p pw_brain -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn talks() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(DEFAULT_DIR);
        let t = Instant::now();
        let mut m = LocalModel::load(&dir, 4).expect("model");
        println!("loaded in {:.1}s", t.elapsed().as_secs_f32());
        let sys = "You are Kai, a colonist. Reply only JSON: {\"thought\":\"...\",\"say\":\"...\",\"do\":\"work|eat|sleep|rest|wander|talk\"}";
        for think in [0, 320] {
            let g = m.chat(sys, "You are hungry and it is evening. Ugo is next to you. What do you do?", &GenOptions { think_tokens: think, seed: 3, ..Default::default() }).unwrap();
            println!("think={think}: {} tokens in {:.1}s ({:.1} tok/s)\n  thinking: {}\n  answer: {}", g.tokens, g.secs, g.tokens as f32 / g.secs, g.thinking.chars().take(200).collect::<String>(), g.text);
            assert!(g.text.contains('{'), "no JSON: {}", g.text);
        }
    }
}
