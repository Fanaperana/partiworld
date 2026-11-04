//! The whole simulation in one place: `Game` owns content, world, colony and
//! scripts, runs fixed ticks, streams chunks, and exposes the player's tools.
//! Nothing here depends on Bevy, so it runs headless in tests.

pub mod env;
pub mod script;

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use pw_colony::{Colony, ColonyEvent, nearest_tile};
use pw_life::{Life, LifeEnv, LifeEvent};
use pw_core::*;
use pw_mods::ScriptHost;
use pw_objects::{self as objs, ObjectEvent};
use pw_sim::{SimEnv, SimSettings};
use pw_world::*;
use pw_worldgen::{GenPool, Generator};

pub use pw_colony::TPS;
pub use env::{Env, HOUR_TICKS, Light};
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
    /// Weather, temperature, snow.
    pub env: Env,
    /// Wild animals: their genes, brains and species.
    pub life: Life,
    /// Why the last `build` call skipped tiles, for the UI.
    pub last_build_error: Option<String>,
    /// Recent flashes (x, y, radius, tick) for the renderer: lightning, meteors.
    pub flashes: Vec<(f32, f32, f32, u64)>,
    last_season: String,
    last_weather: String,
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
            env: Env::default(),
            life: Life::default(),
            last_build_error: None,
            flashes: Vec::new(),
            last_season: String::new(),
            last_weather: String::new(),
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
        // The colony's biome makes it warmer or colder than the season says.
        let (cx, cy) = spawn.center();
        let (_, _, t) = generator.climate(cx, cy);
        self.env.biome_temp = (t - 0.5) * 2.0 * self.content.climate.biome_temp_range;
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
        // Wild life around the landing site: rabbit warrens, a deer herd, and a wolf pack further out.
        let groups = [("core:rabbit", 5, (8, 6)), ("core:rabbit", 5, (-9, 7)), ("core:deer", 5, (10, -9)), ("core:wolf", 3, (-18, -16))];
        for (id, n, (dx, dy)) in groups {
            self.spawn_group(id, spawn.offset(dx, dy), n);
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

    /// Swap in changed content while the world keeps running (live editing,
    /// hot reload). Existing materials, objects, items, jobs and creatures
    /// must keep their place (new ones may be added at the end), because the
    /// world stores them by index.
    pub fn swap_content(&mut self, content: Content) -> Result<(), String> {
        let old = &self.content;
        let prefix = |what: &str, a: Vec<String>, b: Vec<String>| -> Result<(), String> {
            if b.len() >= a.len() && a.iter().zip(&b).all(|(x, y)| x == y) {
                Ok(())
            } else {
                Err(format!("{what} were removed or reordered; start a new world to use this change"))
            }
        };
        prefix("materials", old.materials.ids(), content.materials.ids())?;
        prefix("objects", old.objects.ids(), content.objects.ids())?;
        prefix("items", old.items.ids(), content.items.ids())?;
        prefix("jobs", old.jobs.ids(), content.jobs.ids())?;
        prefix("creatures", old.creatures.ids(), content.creatures.ids())?;
        let content = Arc::new(content);
        // Colours, walk costs and reactions may have changed: refresh every chunk.
        for ch in self.world.chunks.values_mut() {
            ch.after_load(&content);
        }
        self.content = content;
        Ok(())
    }

    /// Reload every mod from disk into the running world (hot reload).
    pub fn reload_mods(&mut self) -> Result<String, String> {
        let (content, mut scripts, report) = pw_mods::load_all(&self.mods_dir);
        if !report.ok() {
            return Err(report.errors.join(" | "));
        }
        self.swap_content(content)?;
        script::register_api(&mut scripts.engine);
        for f in &self.ext.script_setup {
            f(&mut scripts.engine);
        }
        self.scripts = scripts;
        Ok(format!("Reloaded {} mods", self.content.mods.len()))
    }

    /// Replace content (e.g. after the editor added an object). Material ids must stay stable.
    pub fn set_content(&mut self, content: Arc<Content>) {
        self.content = content;
    }

    // ------------------------------------------------------------ simulation

    pub fn step(&mut self) {
        let content = self.content.clone();
        self.world.rebuild_tiles(&content);
        self.env.tick(&content.climate, self.tick, &mut self.rng);
        let events = self.colony.tick(&mut self.world, &content, &mut self.rng, self.tick, self.env.walk_speed());
        let sim_env = SimEnv { wet: self.env.wetness.max(self.env.precip * 0.8), wind: self.env.wind_vec(), temp: self.env.temp };
        pw_sim::tick(&mut self.world, &content, &mut self.rng, &self.sim, &sim_env);
        let mut obj_events = objs::process_damage(&mut self.world, &content);
        let life_env = LifeEnv {
            tick: self.tick,
            day_ticks: DAY_TICKS,
            temp: self.env.temp,
            dark: self.light().dark,
            walk: self.env.walk_speed(),
        };
        let life_events = self.life.tick(&mut self.world, &content, &mut self.rng, &life_env);
        self.handle_life(life_events);
        if self.tick % DAY_TICKS == DAY_TICKS / 2 {
            self.migrate();
        }
        let growth = self.env.growth(&content.climate, self.tick);
        obj_events.extend(objs::tick_life(&mut self.world, &content, self.tick, &mut self.rng, growth));
        if self.tick % 30 == 0 {
            self.collapse_roofs();
        }
        self.world_events();

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

    /// Time, season and weather events for scripts.
    fn world_events(&mut self) {
        if self.scripts.asts.is_empty() {
            return;
        }
        if self.tick % env::HOUR_TICKS == 0 {
            self.fire_event("on_hour", vec![]);
        }
        if self.tick % DAY_TICKS == 0 {
            let day = (self.tick / DAY_TICKS + 1) as i64;
            self.fire_event("on_day", vec![day.into()]);
        }
        let season = self.season_name().to_string();
        if self.last_season != season {
            if !self.last_season.is_empty() {
                self.fire_event("on_season", vec![season.clone().into()]);
            }
            self.last_season = season;
        }
        if self.last_weather != self.env.weather {
            if !self.last_weather.is_empty() {
                self.fire_event("on_weather", vec![self.env.weather.clone().into()]);
            }
            self.last_weather = self.env.weather.clone();
        }
    }

    /// Roofs that lost their wall or rock support fall in and leave debris.
    fn collapse_roofs(&mut self) {
        let content = self.content.clone();
        // Roofs still under construction are left alone.
        let building: HashSet<ObjId> = self.colony.blueprints.values().filter_map(|b| b.obj).collect();
        let fallen: Vec<_> = objs::unsupported_roofs(&self.world, &content).into_iter().filter(|r| !building.contains(&r.0)).collect();
        if fallen.is_empty() {
            return;
        }
        for &(id, def, tile) in &fallen {
            if let Some(ObjectEvent::Destroyed { center, .. }) = objs::destroy(&mut self.world, &content, id) {
                if let Some(b) = &content.protos[def].build {
                    for &(item, n) in &b.cost {
                        let back = n / 2;
                        if back > 0 {
                            self.colony.items.add(&self.world, &content, item, back, tile);
                        }
                    }
                }
                self.run_hook(def, "on_destroyed", center);
            }
        }
        self.message(format!("{} roof tile(s) collapsed: no wall or rock close enough to hold them up.", fallen.len()));
    }

    /// Year, season name, day of season, hour.
    pub fn calendar(&self) -> env::Calendar {
        env::calendar(&self.content.climate, self.tick)
    }

    pub fn season_name(&self) -> &str {
        let c = self.calendar();
        self.content.climate.seasons.get(c.season).map(|s| s.name.as_str()).unwrap_or("")
    }

    pub fn light(&self) -> Light {
        env::light(&self.content.climate, self.tick, self.env.clouds)
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
            // Never request an absurd area: keep at most 18 x 12 chunks around the view's centre.
            let (cx, cy) = ((a.x + b.x) / 2, (a.y + b.y) / 2);
            let a = ChunkPos::new(a.x.max(cx - 9), a.y.max(cy - 6));
            let b = ChunkPos::new(b.x.min(cx + 9), b.y.min(cy + 6));
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
        self.last_build_error = None;
        for t in tiles {
            let seed = hash2(self.seed, t.x as i64, t.y as i64);
            match self.colony.place_blueprint(&self.world, &self.content, d, t, seed) {
                Ok(()) => n += 1,
                Err(e) => self.last_build_error = Some(e),
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

    /// Use a god power (from `powers/` data) centred on a particle.
    pub fn use_power(&mut self, id: &str, cx: i32, cy: i32, radius: i32) -> bool {
        let content = self.content.clone();
        let Some(pi) = content.powers.idx(id) else { return false };
        let power = content.powers.by_idx(pi).clone();
        let owner = content.powers.source(pi).to_string();
        let radius = radius.max(0);
        let rf = radius.max(1) as f32;
        let fire = content.mat("core:fire");
        let flammable = content.mat_mask("tag:flammable").unwrap_or_default();
        for effect in &power.effects {
            match effect {
                PowerEffect::Paint { layer, set, chance, onto, ring } => {
                    let Some(m) = content.mat(set) else {
                        self.message(format!("Power {id}: unknown material '{set}'"));
                        continue;
                    };
                    let mask = onto.as_deref().and_then(|o| content.mat_mask(o));
                    let reach = (ring.1 * rf).ceil() as i32;
                    for y in -reach..=reach {
                        for x in -reach..=reach {
                            let d = ((x * x + y * y) as f32).sqrt() / rf;
                            if d < ring.0 || d > ring.1 || (*chance < 1.0 && !self.rng.chance(*chance)) {
                                continue;
                            }
                            let (px, py) = (cx + x, cy + y);
                            if let Some(mask) = &mask {
                                let cur = self.world.get(px, py, *layer);
                                if !mask.get(cur as usize).copied().unwrap_or(false) {
                                    continue;
                                }
                            }
                            self.world.set(&content, px, py, *layer, m, NO_OBJ);
                        }
                    }
                }
                PowerEffect::Ignite { chance, ring } => {
                    let Some(fire) = fire else { continue };
                    let reach = (ring.1 * rf).ceil() as i32;
                    for y in -reach..=reach {
                        for x in -reach..=reach {
                            let d = ((x * x + y * y) as f32).sqrt() / rf;
                            if d < ring.0 || d > ring.1 || !self.rng.chance(*chance) {
                                continue;
                            }
                            let (px, py) = (cx + x, cy + y);
                            for l in [Layer::Overlay, Layer::Solid, Layer::Floor] {
                                let m = self.world.get(px, py, l);
                                if m != NONE_MAT && flammable.get(m as usize).copied().unwrap_or(false) {
                                    self.world.set(&content, px, py, l, fire, NO_OBJ);
                                    break;
                                }
                            }
                        }
                    }
                }
                PowerEffect::Place { object, count } => {
                    for _ in 0..*count {
                        let (x, y) = self.random_in(cx, cy, radius);
                        if let Some(d) = content.objects.idx(object) {
                            let seed = self.rng.next_u64();
                            let origin = match content.protos[d].anchor {
                                Anchor::Center => (x, y),
                                Anchor::Tile => tile_of(x, y).origin(),
                            };
                            if objs::can_place(&self.world, &content, d, origin, seed) {
                                objs::place(&mut self.world, &content, d, origin, seed, false);
                            }
                        }
                    }
                }
                PowerEffect::Spawn { creature, count } => {
                    for _ in 0..*count {
                        let (x, y) = self.random_in(cx, cy, radius);
                        self.spawn_creature(creature, tile_of(x, y));
                    }
                }
                PowerEffect::Weather { id: w } => {
                    if content.climate.weather.iter().any(|d| &d.id == w) {
                        self.env.weather = w.clone();
                        self.env.weather_until = self.tick + 6 * env::HOUR_TICKS;
                        if let Some(d) = content.climate.weather.iter().find(|d| &d.id == w) {
                            self.env.wind_target = d.wind.1;
                        }
                    }
                }
                PowerEffect::Script { function } => self.call_script(&owner, function, (cx, cy)),
                PowerEffect::Flash => self.flashes.push((cx as f32, cy as f32, rf.max(4.0) * 2.5, self.tick)),
            }
        }
        self.flashes.retain(|f| self.tick.saturating_sub(f.3) < 60);
        true
    }

    fn random_in(&mut self, cx: i32, cy: i32, r: i32) -> (i32, i32) {
        let a = self.rng.f32() * std::f32::consts::TAU;
        let d = self.rng.f32().sqrt() * r.max(1) as f32;
        (cx + (a.cos() * d) as i32, cy + (a.sin() * d) as i32)
    }

    /// Spawn any creature by id: wild animals join the wild, colonists the colony.
    pub fn spawn_creature(&mut self, id: &str, t: TilePos) -> bool {
        let Some(c) = self.content.creatures.idx(id) else { return false };
        let Some(t) = nearest_tile(t, 3, |t| self.world.walkable(t)) else { return false };
        if self.content.creatures.by_idx(c).life.is_some() {
            let content = self.content.clone();
            return self.life.spawn(&content, c, t, &mut self.rng, self.tick, DAY_TICKS).is_some();
        }
        self.colony.spawn_pawn(&self.content, c, t, &mut self.rng);
        true
    }

    /// Spawn a few creatures around a tile.
    pub fn spawn_group(&mut self, id: &str, at: TilePos, n: u32) -> u32 {
        let mut made = 0;
        for _ in 0..n * 3 {
            if made >= n {
                break;
            }
            let t = at.offset(self.rng.range_i32(-3, 3), self.rng.range_i32(-3, 3));
            if self.spawn_creature(id, t) {
                made += 1;
            }
        }
        made
    }

    fn handle_life(&mut self, events: Vec<LifeEvent>) {
        let content = self.content.clone();
        for e in events {
            match e {
                LifeEvent::Died { def, pos, cause, .. } => {
                    if cause == pw_life::Death::Burnt {
                        continue;
                    }
                    if let Some(life) = &content.creatures.by_idx(def).life {
                        let t = tile_of_f(pos.0, pos.1);
                        for (item, n) in &life.remains {
                            if let Some(i) = content.items.idx(item) {
                                self.colony.items.add(&self.world, &content, i, *n, t);
                            }
                        }
                    }
                }
                LifeEvent::NewSpecies { name, from, .. } => {
                    self.message(format!("A new species has appeared: {name}, evolved from {from}."));
                    self.fire_event("on_new_species", vec![name.into()]);
                }
                LifeEvent::Extinct { name } => {
                    self.message(format!("The {name} have died out here."));
                    self.fire_event("on_extinct", vec![name.into()]);
                }
            }
        }
    }

    /// Animals that died out locally wander back in from the wilds.
    fn migrate(&mut self) {
        let content = self.content.clone();
        let Some(home) = self.colony.pawns.first().map(|p| p.tile()) else { return };
        for (i, d) in content.creatures.iter() {
            let Some(life) = &d.life else { continue };
            if !life.migrates || self.life.count_def(i) >= 3 {
                continue;
            }
            let a = self.rng.f32() * std::f32::consts::TAU;
            let r = 30.0;
            let at = home.offset((a.cos() * r) as i32, (a.sin() * r) as i32);
            if self.spawn_group(&d.id, at, 3) > 0 {
                self.message(format!("{} wander in from the wilds.", d.name));
            }
        }
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
        if let Some(c) = self.life.at((px as f32, py as f32), 6.0) {
            let sp = self.life.species_of(c.species).map(|s| s.name.clone()).unwrap_or_default();
            let days = c.age as f32 / DAY_TICKS as f32;
            out.push(format!("{sp}, {days:.1} days old, generation {}", c.generation));
            out.push(format!("{} - energy {:.0}", c.activity(), c.energy));
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
        let Some(ast) = self.scripts.resolve(owner, name, 3).cloned() else {
            self.message(format!("Script function '{name}' not found"));
            return;
        };
        let fname = ScriptHost::fn_name(name).to_string();
        self.run_script(&ast, &fname, vec![(at.0 as i64).into(), (at.1 as i64).into()]);
    }

    /// What scripts can read about the world right now.
    pub fn world_info(&self) -> script::WorldInfo {
        let cal = self.calendar();
        let mut counts: std::collections::HashMap<String, i64> = Default::default();
        for c in &self.life.creatures {
            *counts.entry(self.content.creatures.by_idx(c.def).id.clone()).or_insert(0) += 1;
        }
        for p in &self.colony.pawns {
            *counts.entry(self.content.creatures.by_idx(p.creature).id.clone()).or_insert(0) += 1;
        }
        let colony = self.colony.pawns.first().map(|p| (p.pos.0 as i32, p.pos.1 as i32)).unwrap_or((0, 0));
        script::WorldInfo {
            temp: self.env.temp,
            season: self.season_name().to_string(),
            weather: self.env.weather.clone(),
            hour: cal.hour,
            day: (self.tick / DAY_TICKS + 1) as i64,
            year: cal.year as i64,
            tick: self.tick as i64,
            colony,
            counts,
        }
    }

    /// Call every mod's `fn <event>(ctx, ...)` if it has one (world events).
    pub fn fire_event(&mut self, event: &str, args: Vec<rhai::Dynamic>) {
        let arity = args.len() + 1;
        let asts: Vec<rhai::AST> = self
            .scripts
            .asts
            .values()
            .filter(|a| a.iter_functions().any(|f| f.name == event && f.params.len() == arity))
            .cloned()
            .collect();
        for ast in asts {
            self.run_script(&ast, event, args.clone());
        }
    }

    fn run_script(&mut self, ast: &rhai::AST, name: &str, args: Vec<rhai::Dynamic>) {
        let info = self.world_info();
        let world = std::mem::take(&mut self.world);
        let ctx = ScriptCtx::with_info(world, self.content.clone(), self.rng.next_u64(), info);
        let mut all: Vec<rhai::Dynamic> = vec![rhai::Dynamic::from(ctx.clone())];
        all.extend(args);
        let result = self.scripts.engine.call_fn::<rhai::Dynamic>(&mut rhai::Scope::new(), ast, name, all);
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
                ScriptCmd::Spawn { creature, x, y, count } => {
                    self.spawn_group(&creature, tile_of(x, y), count);
                }
                ScriptCmd::Power { id, x, y, radius } => {
                    if !self.use_power(&id, x, y, radius) {
                        self.message(format!("Script: unknown power '{id}'"));
                    }
                }
                ScriptCmd::Weather(id) => {
                    let content = self.content.clone();
                    if let Some(d) = content.climate.weather.iter().find(|d| d.id == id) {
                        self.env.weather = id;
                        self.env.weather_until = self.tick + 6 * env::HOUR_TICKS;
                        self.env.wind_target = d.wind.1;
                    }
                }
            }
        }
    }
}
