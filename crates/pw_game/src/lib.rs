//! The whole simulation in one place: `Game` owns content, world, colony and
//! scripts, runs fixed ticks, streams chunks, and exposes the player's tools.
//! Nothing here depends on Bevy, so it runs headless in tests.

pub mod script;

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use pw_colony::{Colony, ColonyEvent, nearest_tile};
use pw_core::*;
use pw_mods::ScriptHost;
use pw_objects::{self as objs, ObjectEvent};
use pw_sim::SimSettings;
use pw_world::*;
use pw_worldgen::{GenPool, Generator};

pub use pw_colony::TPS;
pub use script::{ScriptCmd, ScriptCtx};

/// Ticks in one in-game day.
pub const DAY_TICKS: u64 = 30 * 60 * 6;

/// A Rust mod. Register extra per-tick systems or script functions here.
pub trait PwPlugin: Send + Sync {
    fn id(&self) -> &str;
    fn build(&self, ext: &mut Extensions);
}

type TickFn = Box<dyn Fn(&mut Game) + Send + Sync>;
type EngineFn = Box<dyn Fn(&mut rhai::Engine) + Send + Sync>;

#[derive(Default)]
pub struct Extensions {
    /// Run after every simulation tick.
    pub tick_systems: Vec<(String, TickFn)>,
    /// Add functions/types to the Rhai engine.
    pub script_setup: Vec<EngineFn>,
}

impl Extensions {
    pub fn from_plugins(plugins: &[Box<dyn PwPlugin>]) -> Self {
        let mut e = Extensions::default();
        for p in plugins {
            p.build(&mut e);
        }
        e
    }
    pub fn add_tick_system(&mut self, name: &str, f: impl Fn(&mut Game) + Send + Sync + 'static) {
        self.tick_systems.push((name.to_string(), Box::new(f)));
    }
}

pub struct Game {
    pub content: Arc<Content>,
    pub scripts: ScriptHost,
    pub ext: Arc<Extensions>,
    pub world: World,
    pub colony: Colony,
    pub seed: u64,
    pub tick: u64,
    pub rng: Rng,
    pub sim: SimSettings,
    pub log: VecDeque<String>,
    pub mods_dir: PathBuf,
    pool: GenPool,
    /// Chunks wanted by the camera; set by the front end each frame.
    pub view: Option<(ChunkPos, ChunkPos)>,
}

impl Game {
    pub fn new(content: Arc<Content>, mut scripts: ScriptHost, ext: Arc<Extensions>, seed: u64, mods_dir: PathBuf) -> Self {
        script::register_api(&mut scripts.engine);
        for f in &ext.script_setup {
            f(&mut scripts.engine);
        }
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).clamp(1, 4);
        let mut g = Self {
            content,
            scripts,
            ext,
            world: World::new(seed),
            colony: Colony::default(),
            seed,
            tick: 0,
            rng: Rng::new(seed ^ 0xC010_4157),
            sim: SimSettings::default(),
            log: VecDeque::new(),
            mods_dir,
            pool: GenPool::new(threads),
            view: None,
        };
        g.start_colony();
        g
    }

    /// Generate the start area synchronously and drop the first colonists in.
    fn start_colony(&mut self) {
        let generator = Generator::new(self.seed, &self.content);
        // Find a dry, open spot near the origin using terrain only.
        let mut spawn = TilePos::new(0, 0);
        'search: for r in 0..80 {
            for (dx, dy) in [(r, 0), (-r, 0), (0, r), (0, -r), (r, r), (-r, -r), (r, -r), (-r, r)] {
                let t = TilePos::new(dx * 3, dy * 3);
                let ok = (-2..=2).all(|ox| {
                    (-2..=2).all(|oy| {
                        let (x, y) = t.offset(ox, oy).center();
                        let ter = generator.terrain_at(&self.content, x as i32, y as i32);
                        ter.solid == NONE_MAT && self.content.mat_rt(ter.floor).walk_cost > 0.0
                    })
                });
                if ok {
                    spawn = t;
                    break 'search;
                }
            }
        }
        let sc = spawn.chunk();
        for y in -1..=1 {
            for x in -1..=1 {
                let cp = ChunkPos::new(sc.x + x, sc.y + y);
                let g = generator.generate(&self.content, cp);
                objs::integrate(&mut self.world, &self.content, g);
            }
        }
        // Clear a small landing zone so colonists are not inside a tree.
        for t in TileRect::from_corners(spawn.offset(-2, -2), spawn.offset(2, 2)).iter() {
            for o in self.world.objects_in_tile(t) {
                objs::destroy(&mut self.world, &self.content, o);
            }
        }
        self.world.rebuild_tiles(&self.content);
        if let Some(creature) = self.content.creatures.idx("core:colonist").or(if self.content.creatures.is_empty() { None } else { Some(0) }) {
            for i in 0..3 {
                let t = nearest_tile(spawn.offset(i - 1, 0), 6, |t| self.world.walkable(t)).unwrap_or(spawn);
                self.colony.spawn_pawn(&self.content, creature, t, &mut self.rng);
            }
        }
        for (item, n) in [("core:wood", 60), ("core:berries", 30), ("core:stone", 20)] {
            if let Some(i) = self.content.items.idx(item) {
                self.colony.items.add(&self.world, &self.content, i, n, spawn.offset(0, -2));
            }
        }
        self.message(format!("Your colonists land. Seed {}.", self.seed));
    }

    pub fn spawn_tile(&self) -> TilePos {
        self.colony.pawns.first().map(|p| p.tile()).unwrap_or_default()
    }

    pub fn message(&mut self, m: impl Into<String>) {
        self.log.push_back(m.into());
        while self.log.len() > 60 {
            self.log.pop_front();
        }
    }

    /// Replace content (e.g. after the editor added an object). Material ids must stay stable.
    pub fn set_content(&mut self, content: Arc<Content>) {
        self.content = content;
    }

    // ------------------------------------------------------------ simulation

    pub fn step(&mut self) {
        let content = self.content.clone();
        self.world.rebuild_tiles(&content);
        let events = self.colony.tick(&mut self.world, &content, &mut self.rng, self.tick);
        pw_sim::tick(&mut self.world, &content, &mut self.rng, &self.sim);
        let mut obj_events = objs::process_damage(&mut self.world, &content);
        obj_events.extend(objs::tick_life(&mut self.world, &content, self.tick, &mut self.rng));

        for e in events {
            match e {
                ColonyEvent::Destroyed { def, center } => self.run_hook(def, "on_destroyed", center),
                ColonyEvent::Harvested { def, center } => self.run_hook(def, "on_harvested", center),
                ColonyEvent::Built { def, center, .. } => {
                    let name = content.objects.by_idx(def).name.clone();
                    self.message(format!("Built {name}."));
                    self.run_hook(def, "on_built", center)
                }
                ColonyEvent::Message(m) => self.message(m),
            }
        }
        for e in obj_events {
            match e {
                ObjectEvent::Destroyed { def, center, .. } => self.run_hook(def, "on_destroyed", center),
                ObjectEvent::Grown { def, center } => self.run_hook(def, "on_grown", center),
                ObjectEvent::Built { def, center, .. } => self.run_hook(def, "on_built", center),
            }
        }
        let ext = self.ext.clone();
        for (_, f) in &ext.tick_systems {
            f(self);
        }
        self.tick += 1;
    }

    pub fn day_time(&self) -> (u64, u32, u32) {
        let day = self.tick / DAY_TICKS + 1;
        let frac = (self.tick % DAY_TICKS) as f32 / DAY_TICKS as f32;
        let minutes = (frac * 24.0 * 60.0) as u32 + 6 * 60;
        (day, (minutes / 60) % 24, minutes % 60)
    }

    // ------------------------------------------------------------ chunk streaming

    /// Request chunks around the view and colonists; integrate finished ones; drop far unmodified ones.
    pub fn stream_chunks(&mut self) {
        let mut needed: HashSet<ChunkPos> = HashSet::new();
        let add_box = |set: &mut HashSet<ChunkPos>, a: ChunkPos, b: ChunkPos| {
            for y in a.y..=b.y {
                for x in a.x..=b.x {
                    set.insert(ChunkPos::new(x, y));
                }
            }
        };
        if let Some((a, b)) = self.view {
            // Never request an absurd area when zoomed far out.
            let a = ChunkPos::new(a.x.max(b.x - 12), a.y.max(b.y - 10));
            add_box(&mut needed, ChunkPos::new(a.x - 1, a.y - 1), ChunkPos::new(b.x + 1, b.y + 1));
        }
        for p in &self.colony.pawns {
            let c = p.tile().chunk();
            add_box(&mut needed, ChunkPos::new(c.x - 1, c.y - 1), ChunkPos::new(c.x + 1, c.y + 1));
        }
        let mut want: Vec<ChunkPos> = needed.iter().copied().filter(|c| !self.world.chunks.contains_key(c)).collect();
        let center = self.view.map(|(a, b)| ((a.x + b.x) / 2, (a.y + b.y) / 2)).unwrap_or((0, 0));
        want.sort_by_key(|c| (c.x - center.0).abs() + (c.y - center.1).abs());
        for c in want {
            self.pool.request(c, self.seed, self.content.clone());
        }
        for g in self.pool.finished() {
            if !self.world.chunks.contains_key(&g.chunk.pos) {
                objs::integrate(&mut self.world, &self.content, g);
            }
        }
        // Unload far, untouched chunks (they regenerate identically).
        let keep = |c: &ChunkPos| needed.iter().any(|n| (n.x - c.x).abs() <= 2 && (n.y - c.y).abs() <= 2);
        let drop: Vec<ChunkPos> = self.world.chunks.iter().filter(|(c, ch)| !ch.modified && !keep(c)).map(|(c, _)| *c).collect();
        for c in drop {
            self.world.chunks.remove(&c);
        }
    }

    /// Generate chunks synchronously (tests, loading).
    pub fn load_chunks_now(&mut self, a: ChunkPos, b: ChunkPos) {
        let generator = Generator::new(self.seed, &self.content);
        for y in a.y..=b.y {
            for x in a.x..=b.x {
                let cp = ChunkPos::new(x, y);
                if !self.world.chunks.contains_key(&cp) {
                    let g = generator.generate(&self.content, cp);
                    objs::integrate(&mut self.world, &self.content, g);
                }
            }
        }
        self.world.rebuild_tiles(&self.content);
    }

    /// Swap in a loaded world (save games). Chunk requests for the old world are dropped.
    pub fn replace_state(&mut self, world: World, colony: Colony, seed: u64, tick: u64, rng: Rng) {
        self.world = world;
        self.colony = colony;
        self.seed = seed;
        self.tick = tick;
        self.rng = rng;
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).clamp(1, 4);
        self.pool = GenPool::new(threads);
        self.world.rebuild_tiles(&self.content.clone());
    }

    pub fn pending_chunks(&self) -> usize {
        self.pool.pending.len()
    }

    // ------------------------------------------------------------ player tools

    pub fn designate(&mut self, job: &str, rect: TileRect, plant: Option<&str>) -> usize {
        let Some(j) = self.content.jobs.idx(job) else { return 0 };
        let plant = plant.and_then(|p| self.content.objects.idx(p));
        self.colony.designate(&self.world, &self.content, j, rect, plant)
    }

    pub fn cancel(&mut self, rect: TileRect) {
        let content = self.content.clone();
        self.colony.cancel(&mut self.world, &content, rect);
    }

    pub fn stockpile(&mut self, rect: TileRect, on: bool) {
        self.colony.set_stockpile(&self.world, rect, on);
    }

    /// Place blueprints. Walls (tag `wall`) are drawn as a room outline; everything else fills the area.
    pub fn build(&mut self, def: &str, rect: TileRect) -> usize {
        let Some(d) = self.content.objects.idx(def) else { return 0 };
        let outline = self.content.protos[d].has_tag("wall") && rect.width() > 2 && rect.height() > 2;
        let mut n = 0;
        let tiles: Vec<TilePos> = rect
            .iter()
            .filter(|t| !outline || t.x == rect.min.x || t.x == rect.max.x || t.y == rect.min.y || t.y == rect.max.y)
            .collect();
        for t in tiles {
            let seed = hash2(self.seed, t.x as i64, t.y as i64);
            if self.colony.place_blueprint(&self.world, &self.content, d, t, seed) {
                n += 1;
            }
        }
        n
    }

    /// God tool: paint particles in a disc.
    pub fn paint(&mut self, cx: i32, cy: i32, radius: i32, layer: Layer, mat: &str) {
        let Some(m) = self.content.mat(mat) else { return };
        let content = self.content.clone();
        for y in -radius..=radius {
            for x in -radius..=radius {
                if x * x + y * y <= radius * radius {
                    self.world.set(&content, cx + x, cy + y, layer, m, NO_OBJ);
                }
            }
        }
    }

    /// God tool: drop an object anywhere (ignores placement rules except loaded ground).
    pub fn place_object(&mut self, def: &str, x: i32, y: i32) -> bool {
        let Some(d) = self.content.objects.idx(def) else { return false };
        if !self.world.is_loaded(x, y) {
            return false;
        }
        let seed = self.rng.next_u64();
        let origin = match self.content.protos[d].anchor {
            Anchor::Center => (x, y),
            Anchor::Tile => tile_of(x, y).origin(),
        };
        objs::place(&mut self.world, &self.content, d, origin, seed, false);
        true
    }

    /// God tool: start a fire.
    pub fn ignite(&mut self, cx: i32, cy: i32, radius: i32) {
        let Some(fire) = self.content.mat("core:fire") else { return };
        let content = self.content.clone();
        for y in -radius..=radius {
            for x in -radius..=radius {
                if x * x + y * y > radius * radius {
                    continue;
                }
                let (px, py) = (cx + x, cy + y);
                for l in [Layer::Overlay, Layer::Solid, Layer::Floor] {
                    let m = self.world.get(px, py, l);
                    if m != NONE_MAT && content.mat_rt(m).has_tag("flammable") {
                        self.world.set(&content, px, py, l, fire, NO_OBJ);
                        break;
                    }
                }
            }
        }
    }

    pub fn spawn_colonist(&mut self, t: TilePos) {
        if let Some(c) = self.content.creatures.idx("core:colonist") {
            if self.world.walkable(t) {
                self.colony.spawn_pawn(&self.content, c, t, &mut self.rng);
            }
        }
    }

    /// Text for the hover panel.
    pub fn describe(&self, px: i32, py: i32) -> Vec<String> {
        let mut out = Vec::new();
        let t = tile_of(px, py);
        out.push(format!("Tile {}, {}  (particle {px}, {py})", t.x, t.y));
        if !self.world.is_loaded(px, py) {
            out.push("Unexplored".into());
            return out;
        }
        let c = &self.content;
        for l in [Layer::Overlay, Layer::Solid, Layer::Floor] {
            let m = self.world.get(px, py, l);
            if m != NONE_MAT {
                out.push(format!("{}: {}", l.name(), c.mat_name(m)));
            }
        }
        let mut seen = Vec::new();
        for l in Layer::ALL {
            let o = self.world.get_obj(px, py, l);
            if o != NO_OBJ && !seen.contains(&o) {
                seen.push(o);
                if let Some(inst) = self.world.objects.get(o) {
                    out.push(format!(
                        "{} ({:.0}% intact){}",
                        c.objects.by_idx(inst.def).name,
                        inst.integrity() * 100.0,
                        if inst.built { ", built" } else { "" }
                    ));
                }
            }
        }
        if let Some(info) = self.world.tile(t) {
            out.push(if info.walkable { format!("Walkable, cost {:.1}", info.cost) } else { "Blocked".into() });
        }
        if let Some(s) = self.colony.items.at(t) {
            out.push(format!("{} x{}", c.items.by_idx(s.item).name, s.count));
        }
        if let Some(d) = self.colony.designations.get(&t) {
            out.push(format!("Order: {}", c.jobs.by_idx(d.job).label));
        }
        if self.colony.stockpile.contains(&t) {
            out.push("Stockpile".into());
        }
        out
    }

    // ------------------------------------------------------------ scripts

    /// Call an object's script hook (`on_destroyed`, ...) with (ctx, x, y).
    pub fn run_hook(&mut self, def: usize, hook: &str, center: (i32, i32)) {
        let Some(name) = self.content.protos[def].hooks.get(hook).cloned() else { return };
        let owner = self.content.objects.source(def).to_string();
        self.call_script(&owner, &name, center);
    }

    pub fn call_script(&mut self, owner: &str, name: &str, at: (i32, i32)) {
        let Some(ast) = self.scripts.resolve(owner, name, 3) else {
            self.message(format!("Script function '{name}' not found"));
            return;
        };
        let world = std::mem::take(&mut self.world);
        let ctx = ScriptCtx::new(world, self.content.clone(), self.rng.next_u64());
        let result = self.scripts.engine.call_fn::<rhai::Dynamic>(
            &mut rhai::Scope::new(),
            ast,
            ScriptHost::fn_name(name),
            (ctx.clone(), at.0 as i64, at.1 as i64),
        );
        let (world, cmds) = ctx.finish();
        self.world = world;
        if let Err(e) = result {
            self.message(format!("Script '{name}' failed: {e}"));
        }
        for c in cmds {
            match c {
                ScriptCmd::SpawnItem { x, y, item, count } => {
                    if let Some(i) = self.content.items.idx(&item) {
                        let content = self.content.clone();
                        self.colony.items.add(&self.world, &content, i, count, tile_of(x, y));
                    } else {
                        self.message(format!("Script: unknown item '{item}'"));
                    }
                }
                ScriptCmd::PlaceObject { x, y, def } => {
                    if !self.place_object(&def, x, y) {
                        self.message(format!("Script: cannot place '{def}'"));
                    }
                }
                ScriptCmd::Message(m) => self.message(m),
            }
        }
    }
}
