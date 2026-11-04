//! The Rhai API mod scripts see. A hook is `fn name(ctx, x, y)`:
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
}

struct State {
    world: World,
    content: Arc<Content>,
    rng: Rng,
    cmds: Vec<ScriptCmd>,
}

/// Handle passed to scripts. Holds the world while a hook runs.
#[derive(Clone)]
pub struct ScriptCtx(Arc<Mutex<State>>);

impl ScriptCtx {
    pub fn new(world: World, content: Arc<Content>, seed: u64) -> Self {
        Self(Arc::new(Mutex::new(State { world, content, rng: Rng::new(seed), cmds: Vec::new() })))
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
    engine.register_fn("rand_int", |c: &mut ScriptCtx, lo: i64, hi: i64| -> i64 {
        c.with(|s| s.rng.range_i32(lo as i32, hi as i32) as i64)
    });
}
