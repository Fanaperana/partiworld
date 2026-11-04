//! Mod loading. A mod is a folder with a `mod.toml` and any of these folders:
//! `materials/ items/ objects/ biomes/ reactions/ jobs/ creatures/` (RON files,
//! each holding one def or a list of defs), `worldgen.ron`, `patches/*.json`
//! and `scripts/*.rhai`.
//!
//! Load order follows `depends` and `load_after`. A def whose id already exists
//! replaces the earlier one (override). Patches change single fields of
//! existing defs without copying the whole def.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use pw_core::*;
use rhai::{AST, Engine};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// Compiled Rhai scripts, one AST per mod.
pub struct ScriptHost {
    pub engine: Engine,
    pub asts: BTreeMap<String, AST>,
}

impl Default for ScriptHost {
    fn default() -> Self {
        let mut engine = Engine::new();
        engine.set_max_operations(2_000_000);
        engine.set_max_call_levels(64);
        Self { engine, asts: BTreeMap::new() }
    }
}

impl ScriptHost {
    /// Find which mod's script defines `name` (with `arity` params). The owner mod is searched first.
    pub fn resolve(&self, owner: &str, name: &str, arity: usize) -> Option<&AST> {
        let (want_mod, fname) = match name.split_once("::") {
            Some((m, f)) => (Some(m), f),
            None => (None, name),
        };
        let has = |ast: &AST| ast.iter_functions().any(|f| f.name == fname && f.params.len() == arity);
        if let Some(m) = want_mod {
            return self.asts.get(m).filter(|a| has(a));
        }
        if let Some(a) = self.asts.get(owner).filter(|a| has(a)) {
            return Some(a);
        }
        self.asts.values().find(|a| has(a))
    }

    pub fn fn_name(name: &str) -> &str {
        name.split_once("::").map(|(_, f)| f).unwrap_or(name)
    }
}

#[derive(Debug, Default)]
pub struct LoadReport {
    pub loaded: Vec<String>,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

impl LoadReport {
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// `patches/*.json`: `[{"target": "object:core:oak_tree", "set": {...}, "append": {...}}]`
#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Patch {
    pub target: String,
    #[serde(default)]
    pub set: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    pub append: serde_json::Map<String, serde_json::Value>,
}

pub fn discover(mods_dir: &Path) -> (Vec<ModInfo>, Vec<String>) {
    let mut out = Vec::new();
    let mut errs = Vec::new();
    let Ok(rd) = fs::read_dir(mods_dir) else {
        errs.push(format!("mods folder not found: {}", mods_dir.display()));
        return (out, errs);
    };
    let mut dirs: Vec<PathBuf> = rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    for d in dirs {
        let manifest = d.join("mod.toml");
        let Ok(text) = fs::read_to_string(&manifest) else { continue };
        match toml::from_str::<ModInfo>(&text) {
            Ok(mut m) => {
                m.path = d.clone();
                out.push(m);
            }
            Err(e) => errs.push(format!("{}: {e}", manifest.display())),
        }
    }
    (out, errs)
}

/// Topological sort by `depends` + `load_after`; stable by folder order otherwise.
pub fn sort_mods(mods: Vec<ModInfo>) -> Result<Vec<ModInfo>, Vec<String>> {
    let mut errs = Vec::new();
    let ids: HashSet<String> = mods.iter().map(|m| m.id.clone()).collect();
    for m in &mods {
        for d in &m.depends {
            if !ids.contains(d) {
                errs.push(format!("mod '{}' needs missing mod '{}'", m.id, d));
            }
        }
    }
    if !errs.is_empty() {
        return Err(errs);
    }
    let pos: HashMap<String, usize> = mods.iter().enumerate().map(|(i, m)| (m.id.clone(), i)).collect();
    let before = |m: &ModInfo| -> Vec<usize> {
        m.depends.iter().chain(m.load_after.iter()).filter_map(|d| pos.get(d).copied()).collect()
    };
    let mut done = vec![false; mods.len()];
    let mut order = Vec::new();
    while order.len() < mods.len() {
        let next = (0..mods.len()).find(|&i| !done[i] && before(&mods[i]).iter().all(|&b| done[b]));
        match next {
            Some(i) => {
                done[i] = true;
                order.push(i);
            }
            None => {
                let stuck: Vec<_> = (0..mods.len()).filter(|&i| !done[i]).map(|i| mods[i].id.clone()).collect();
                return Err(vec![format!("dependency cycle between mods: {}", stuck.join(", "))]);
            }
        }
    }
    let mut slots: Vec<Option<ModInfo>> = mods.into_iter().map(Some).collect();
    Ok(order.into_iter().map(|i| slots[i].take().unwrap()).collect())
}

fn ron_files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "ron"))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// First real token (skipping `//` comment lines) is `[`.
fn starts_with_list(text: &str) -> bool {
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with("//") {
            continue;
        }
        return t.starts_with('[');
    }
    false
}

fn ron_opts() -> ron::Options {
    ron::Options::default()
        .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
        .with_default_extension(ron::extensions::Extensions::UNWRAP_VARIANT_NEWTYPES)
}

pub fn parse_ron<T: DeserializeOwned>(text: &str) -> Result<T, String> {
    ron_opts().from_str::<T>(text).map_err(|e| e.to_string())
}

fn load_dir<T: DeserializeOwned + Def>(
    m: &ModInfo,
    folder: &str,
    reg: &mut Registry<T>,
    report: &mut LoadReport,
) {
    for f in ron_files(&m.path.join(folder)) {
        let Ok(text) = fs::read_to_string(&f) else {
            report.errors.push(format!("{}: cannot read", f.display()));
            continue;
        };
        // A file holds either a list of defs or a single def.
        let list = if starts_with_list(&text) {
            parse_ron::<Vec<T>>(&text)
        } else {
            parse_ron::<T>(&text).map(|d| vec![d])
        };
        match list {
            Ok(list) => {
                for d in list {
                    reg.insert(d, &m.id);
                }
            }
            Err(e) => report.errors.push(format!("{}: {e}", f.display())),
        }
    }
}

fn apply_patch<T: Serialize + DeserializeOwned + Def>(reg: &mut Registry<T>, id: &str, p: &Patch) -> Result<(), String> {
    let cur = reg.get(id).ok_or(format!("patch target '{id}' not found"))?;
    let mut v = serde_json::to_value(cur).map_err(|e| e.to_string())?;
    let obj = v.as_object_mut().ok_or("def is not an object")?;
    for (k, val) in &p.set {
        obj.insert(k.clone(), val.clone());
    }
    for (k, val) in &p.append {
        let arr = obj.entry(k.clone()).or_insert(serde_json::Value::Array(vec![]));
        let arr = arr.as_array_mut().ok_or(format!("field '{k}' is not a list"))?;
        match val {
            serde_json::Value::Array(xs) => arr.extend(xs.iter().cloned()),
            x => arr.push(x.clone()),
        }
    }
    let new: T = serde_json::from_value(v).map_err(|e| format!("patch on '{id}': {e}"))?;
    let src = reg.idx(id).map(|i| reg.source(i).to_string()).unwrap_or_default();
    reg.insert(new, &src);
    Ok(())
}

fn load_patches(m: &ModInfo, c: &mut Content, report: &mut LoadReport) {
    let dir = m.path.join("patches");
    let Ok(rd) = fs::read_dir(&dir) else { return };
    let mut files: Vec<PathBuf> =
        rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
    files.sort();
    for f in files {
        let text = fs::read_to_string(&f).unwrap_or_default();
        let patches: Vec<Patch> = match serde_json::from_str(&text) {
            Ok(p) => p,
            Err(e) => {
                report.errors.push(format!("{}: {e}", f.display()));
                continue;
            }
        };
        for p in patches {
            let Some((kind, id)) = p.target.split_once(':') else {
                report.errors.push(format!("{}: bad target '{}'", f.display(), p.target));
                continue;
            };
            let r = match kind {
                "material" => apply_patch(&mut c.materials, id, &p),
                "item" => apply_patch(&mut c.items, id, &p),
                "object" => apply_patch(&mut c.objects, id, &p),
                "biome" => apply_patch(&mut c.biomes, id, &p),
                "reaction" => apply_patch(&mut c.reactions, id, &p),
                "job" => apply_patch(&mut c.jobs, id, &p),
                "creature" => apply_patch(&mut c.creatures, id, &p),
                _ => Err(format!("unknown patch kind '{kind}'")),
            };
            if let Err(e) = r {
                report.errors.push(format!("{}: {e}", f.display()));
            }
        }
    }
}

fn load_scripts(m: &ModInfo, host: &mut ScriptHost, report: &mut LoadReport) {
    let dir = m.path.join("scripts");
    let Ok(rd) = fs::read_dir(&dir) else { return };
    let mut files: Vec<PathBuf> =
        rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "rhai")).collect();
    files.sort();
    let mut merged: Option<AST> = None;
    for f in files {
        match host.engine.compile_file(f.clone()) {
            Ok(ast) => {
                merged = Some(match merged {
                    Some(a) => a.merge(&ast),
                    None => ast,
                })
            }
            Err(e) => report.errors.push(format!("{}: {e}", f.display())),
        }
    }
    if let Some(ast) = merged {
        host.asts.insert(m.id.clone(), ast);
    }
}

/// Load a single mod's content into `c` (no linking).
pub fn load_mod(m: &ModInfo, c: &mut Content, host: &mut ScriptHost, report: &mut LoadReport) {
    load_dir(m, "items", &mut c.items, report);
    load_dir(m, "materials", &mut c.materials, report);
    load_dir(m, "objects", &mut c.objects, report);
    load_dir(m, "biomes", &mut c.biomes, report);
    load_dir(m, "reactions", &mut c.reactions, report);
    load_dir(m, "jobs", &mut c.jobs, report);
    load_dir(m, "creatures", &mut c.creatures, report);
    load_dir(m, "powers", &mut c.powers, report);
    let wg = m.path.join("worldgen.ron");
    if let Ok(text) = fs::read_to_string(&wg) {
        match parse_ron::<WorldGenDef>(&text) {
            Ok(w) => c.worldgen = w,
            Err(e) => report.errors.push(format!("{}: {e}", wg.display())),
        }
    }
    let cl = m.path.join("climate.ron");
    if let Ok(text) = fs::read_to_string(&cl) {
        match parse_ron::<ClimateDef>(&text) {
            Ok(w) => c.climate = w,
            Err(e) => report.errors.push(format!("{}: {e}", cl.display())),
        }
    }
    load_patches(m, c, report);
    load_scripts(m, host, report);
    c.mods.push(m.clone());
    report.loaded.push(format!("{} {}", m.id, m.version));
}

/// Discover, sort, load and link every mod in `mods_dir`.
pub fn load_all(mods_dir: &Path) -> (Content, ScriptHost, LoadReport) {
    let mut report = LoadReport::default();
    let mut content = Content::new();
    let mut host = ScriptHost::default();
    let (mods, errs) = discover(mods_dir);
    report.errors.extend(errs);
    let mods = match sort_mods(mods) {
        Ok(m) => m,
        Err(e) => {
            report.errors.extend(e);
            return (content, host, report);
        }
    };
    for m in &mods {
        load_mod(m, &mut content, &mut host, &mut report);
    }
    if let Err(e) = content.link() {
        report.errors.extend(e);
    }
    (content, host, report)
}

/// Write an object def into a mod folder (used by the in-game editor).
/// Creates the mod (with a `mod.toml` depending on `core`) if needed.
pub fn save_object_to_mod(mods_dir: &Path, mod_id: &str, def: &ObjectDef) -> Result<PathBuf, String> {
    let dir = mods_dir.join(mod_id);
    let objects = dir.join("objects");
    fs::create_dir_all(&objects).map_err(|e| e.to_string())?;
    let manifest = dir.join("mod.toml");
    if !manifest.exists() {
        let text = format!(
            "id = \"{mod_id}\"\nname = \"My creations\"\nversion = \"0.1.0\"\ndescription = \"Objects made in the in-game editor.\"\ndepends = [\"core\"]\n"
        );
        fs::write(&manifest, text).map_err(|e| e.to_string())?;
    }
    let file_name = def.id.replace(':', "_") + ".ron";
    let path = objects.join(file_name);
    let pretty = ron::ser::PrettyConfig::new().struct_names(true);
    let text = ron::ser::to_string_pretty(def, pretty).map_err(|e| e.to_string())?;
    fs::write(&path, text).map_err(|e| e.to_string())?;
    Ok(path)
}
