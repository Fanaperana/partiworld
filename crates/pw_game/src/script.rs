//! The Rhai API mod scripts see.
//!
//! **Object hooks** are `fn name(ctx, x, y)`, named in an object's `hooks`.
//!
//! **World events** are found by name in any mod's scripts and called for you:
//! `on_hour(ctx)`, `on_day(ctx, day)`, `on_season(ctx, season)`,
//! `on_weather(ctx, weather)`, `on_new_species(ctx, name)`, `on_extinct(ctx, name)`.
//!
//! Reading the world: `get`, `has_tag`, `temp`, `season`, `weather`, `hour`,
//! `day`, `year`, `count(creature_id)`, `colony_x`, `colony_y`, `rand`, `rand_int`.
//! Changing it: `set`, `spawn_item`, `place_object`, `spawn(creature, x, y, n)`,
//! `power(id, x, y, radius)`, `ignite(x, y, radius)`, `set_weather(id)`, `message`.
//!
//! A hook example:
//!
//! ```rhai
//! fn tree_fall(ctx, x, y) {
//!     for i in 0..20 {
//!         let px = x + ctx.rand_int(-6, 6);
//!         let py = y + ctx.rand_int(-6, 6);
//!         if ctx.get(px, py, "floor") == "core:grass" { ctx.set(px, py, "floor", "core:leaf_litter"); }
//!     }
//!     ctx.spawn_item(x, y, "core:wood", 2);
//! }
//! ```

use std::sync::{Arc, Mutex};

use pw_core::*;
use pw_world::{NO_OBJ, World};
use rhai::Engine;

#[derive(Clone, Debug)]
pub enum ScriptCmd {
    SpawnItem { x: i32, y: i32, item: String, count: u32 },
    PlaceObject { x: i32, y: i32, def: String },
    Message(String),
    Spawn { creature: String, x: i32, y: i32, count: u32 },
    Power { id: String, x: i32, y: i32, radius: i32 },
    Weather(String),
}

/// A read-only snapshot of the world's state for scripts.
#[derive(Clone, Debug, Default)]
pub struct WorldInfo {
    pub temp: f32,
    pub season: String,
    pub weather: String,
    pub hour: f32,
    pub day: i64,
    pub year: i64,
    pub tick: i64,
    pub colony: (i32, i32),
    /// Living creatures per creature id (wild and colonists).
    pub counts: std::collections::HashMap<String, i64>,
}

struct State {
    world: World,
    content: Arc<Content>,
    rng: Rng,
    cmds: Vec<ScriptCmd>,
    info: WorldInfo,
}

/// Handle passed to scripts. Holds the world while a hook runs.
#[derive(Clone)]
pub struct ScriptCtx(Arc<Mutex<State>>);

impl ScriptCtx {
    pub fn new(world: World, content: Arc<Content>, seed: u64) -> Self {
        Self::with_info(world, content, seed, WorldInfo::default())
    }

    pub fn with_info(world: World, content: Arc<Content>, seed: u64, info: WorldInfo) -> Self {
        Self(Arc::new(Mutex::new(State { world, content, rng: Rng::new(seed), cmds: Vec::new(), info })))
    }

    /// Give the world back, with queued commands.
    pub fn finish(self) -> (World, Vec<ScriptCmd>) {
        let mut s = self.0.lock().unwrap();
        (std::mem::take(&mut s.world), std::mem::take(&mut s.cmds))
    }

    fn with<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        f(&mut self.0.lock().unwrap())
    }
}

fn layer(s: &str) -> Layer {
    Layer::parse(s).unwrap_or(Layer::Solid)
}

pub fn register_api(engine: &mut Engine) {
    engine.register_type_with_name::<ScriptCtx>("Ctx");
    engine.register_fn("get", |c: &mut ScriptCtx, x: i64, y: i64, l: &str| -> String {
        c.with(|s| s.content.mat_id_str(s.world.get(x as i32, y as i32, layer(l))).to_string())
    });
    engine.register_fn("has_tag", |c: &mut ScriptCtx, x: i64, y: i64, l: &str, tag: &str| -> bool {
        c.with(|s| {
            let m = s.world.get(x as i32, y as i32, layer(l));
            s.content.mat_rt(m).has_tag(tag)
        })
    });
    engine.register_fn("set", |c: &mut ScriptCtx, x: i64, y: i64, l: &str, mat: &str| -> bool {
        c.with(|s| {
            let Some(m) = s.content.mat(mat) else { return false };
            let content = s.content.clone();
            s.world.set(&content, x as i32, y as i32, layer(l), m, NO_OBJ)
        })
    });
    engine.register_fn("spawn_item", |c: &mut ScriptCtx, x: i64, y: i64, item: &str, n: i64| {
        c.with(|s| s.cmds.push(ScriptCmd::SpawnItem { x: x as i32, y: y as i32, item: item.into(), count: n.max(0) as u32 }))
    });
    engine.register_fn("place_object", |c: &mut ScriptCtx, x: i64, y: i64, def: &str| {
        c.with(|s| s.cmds.push(ScriptCmd::PlaceObject { x: x as i32, y: y as i32, def: def.into() }))
    });
    engine.register_fn("message", |c: &mut ScriptCtx, m: &str| c.with(|s| s.cmds.push(ScriptCmd::Message(m.into()))));
    engine.register_fn("rand", |c: &mut ScriptCtx| -> f64 { c.with(|s| s.rng.f32() as f64) });
    // Reading the world.
    engine.register_fn("temp", |c: &mut ScriptCtx| -> f64 { c.with(|s| s.info.temp as f64) });
    engine.register_fn("season", |c: &mut ScriptCtx| -> String { c.with(|s| s.info.season.clone()) });
    engine.register_fn("weather", |c: &mut ScriptCtx| -> String { c.with(|s| s.info.weather.clone()) });
    engine.register_fn("hour", |c: &mut ScriptCtx| -> f64 { c.with(|s| s.info.hour as f64) });
    engine.register_fn("day", |c: &mut ScriptCtx| -> i64 { c.with(|s| s.info.day) });
    engine.register_fn("year", |c: &mut ScriptCtx| -> i64 { c.with(|s| s.info.year) });
    engine.register_fn("tick", |c: &mut ScriptCtx| -> i64 { c.with(|s| s.info.tick) });
    engine.register_fn("colony_x", |c: &mut ScriptCtx| -> i64 { c.with(|s| s.info.colony.0 as i64) });
    engine.register_fn("colony_y", |c: &mut ScriptCtx| -> i64 { c.with(|s| s.info.colony.1 as i64) });
    engine.register_fn("count", |c: &mut ScriptCtx, id: &str| -> i64 { c.with(|s| s.info.counts.get(id).copied().unwrap_or(0)) });
    // Changing it (applied right after the script returns).
    engine.register_fn("spawn", |c: &mut ScriptCtx, creature: &str, x: i64, y: i64, n: i64| {
        c.with(|s| s.cmds.push(ScriptCmd::Spawn { creature: creature.into(), x: x as i32, y: y as i32, count: n.clamp(0, 50) as u32 }))
    });
    engine.register_fn("power", |c: &mut ScriptCtx, id: &str, x: i64, y: i64, r: i64| {
        c.with(|s| s.cmds.push(ScriptCmd::Power { id: id.into(), x: x as i32, y: y as i32, radius: r.clamp(0, 200) as i32 }))
    });
    engine.register_fn("ignite", |c: &mut ScriptCtx, x: i64, y: i64, r: i64| {
        c.with(|s| s.cmds.push(ScriptCmd::Power { id: "core:firestorm".into(), x: x as i32, y: y as i32, radius: r.clamp(0, 60) as i32 }))
    });
    engine.register_fn("set_weather", |c: &mut ScriptCtx, id: &str| c.with(|s| s.cmds.push(ScriptCmd::Weather(id.into()))));
    engine.register_fn("rand_int", |c: &mut ScriptCtx, lo: i64, hi: i64| -> i64 {
        c.with(|s| s.rng.range_i32(lo as i32, hi as i32) as i64)
    });
}
