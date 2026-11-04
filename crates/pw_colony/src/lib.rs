//! Colonists and their work: designations -> jobs -> stages.
//! Job kinds are data (`JobDef`) pointing at built-in drivers, so a mod can add
//! e.g. "cut flowers" as a Fell job targeting the `flower` tag.

pub mod path;

use std::collections::{BTreeMap, BTreeSet, HashMap};

use pw_core::*;
use pw_objects as objs;
use pw_world::*;
use serde::{Deserialize, Serialize};

pub use path::{find_path, nearest_tile};

/// Simulation ticks per second at 1x speed.
pub const TPS: f32 = 30.0;
const MAX_PATH_NODES: usize = 6000;

// ---------------------------------------------------------------- items

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ItemStack {
    pub id: u32,
    pub item: usize,
    pub count: u32,
    pub tile: TilePos,
    pub reserved: Option<u32>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Items {
    pub next: u32,
    pub stacks: BTreeMap<u32, ItemStack>,
    #[serde(skip)]
    by_tile: HashMap<TilePos, u32>,
}

impl Items {
    pub fn rebuild_index(&mut self) {
        self.by_tile = self.stacks.values().map(|s| (s.tile, s.id)).collect();
    }
    pub fn at(&self, t: TilePos) -> Option<&ItemStack> {
        self.by_tile.get(&t).and_then(|id| self.stacks.get(id))
    }
    pub fn get(&self, id: u32) -> Option<&ItemStack> {
        self.stacks.get(&id)
    }

    /// Drop items near a tile, filling existing stacks first, spreading out as needed.
    pub fn add(&mut self, world: &World, content: &Content, item: usize, mut count: u32, near: TilePos) {
        let max = content.item_rt[item].stack;
        let mut guard = 0;
        while count > 0 && guard < 64 {
            guard += 1;
            let found = nearest_tile(near, 8, |t| {
                world.walkable(t)
                    && match self.at(t) {
                        None => true,
                        Some(s) => s.item == item && s.count < max && s.reserved.is_none(),
                    }
            });
            let t = found.unwrap_or(near);
            if let Some(&id) = self.by_tile.get(&t) {
                let s = self.stacks.get_mut(&id).unwrap();
                if s.item == item {
                    let add = if found.is_some() { (max - s.count.min(max)).min(count) } else { count };
                    s.count += add;
                    count -= add;
                    continue;
                }
            }
            self.next += 1;
            let n = count.min(max);
            let id = self.next;
            self.stacks.insert(id, ItemStack { id, item, count: n, tile: t, reserved: None });
            self.by_tile.insert(t, id);
            count -= n;
        }
    }

    /// Take up to `n` from a stack. Returns how many were taken.
    pub fn take(&mut self, id: u32, n: u32) -> u32 {
        let Some(s) = self.stacks.get_mut(&id) else { return 0 };
        let got = s.count.min(n);
        s.count -= got;
        if s.count == 0 {
            let t = s.tile;
            self.stacks.remove(&id);
            if self.by_tile.get(&t) == Some(&id) {
                self.by_tile.remove(&t);
            }
        }
        got
    }

    pub fn total(&self, item: usize) -> u32 {
        self.stacks.values().filter(|s| s.item == item).map(|s| s.count).sum()
    }
}

// ---------------------------------------------------------------- work orders

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum DesTarget {
    Object(ObjId),
    Tile,
    /// Plant this object def here.
    Plant(usize),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Designation {
    pub job: usize,
    pub target: DesTarget,
    pub reserved: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Blueprint {
    pub id: u32,
    pub def: usize,
    pub tile: TilePos,
    pub origin: (i32, i32),
    pub seed: u64,
    pub delivered: Vec<u32>,
    pub incoming: Vec<u32>,
    pub progress: u32,
    pub placed: usize,
    pub obj: Option<ObjId>,
    pub reserved: Option<u32>,
}

impl Blueprint {
    pub fn needs(&self, content: &Content) -> Vec<(usize, u32)> {
        let Some(b) = &content.protos[self.def].build else { return Vec::new() };
        b.cost
            .iter()
            .enumerate()
            .filter_map(|(i, &(item, n))| {
                let have = self.delivered[i] + self.incoming[i];
                (have < n).then_some((item, n - have))
            })
            .collect()
    }
    pub fn ready(&self, content: &Content) -> bool {
        let Some(b) = &content.protos[self.def].build else { return true };
        b.cost.iter().enumerate().all(|(i, &(_, n))| self.delivered[i] >= n)
    }
}

// ---------------------------------------------------------------- pawns

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum JobKind {
    /// Work a designation (fell, mine, harvest, deconstruct, plant).
    Work { at: TilePos, driver: JobDriver },
    HaulToStockpile { stack: u32, dest: TilePos },
    HaulToBlueprint { stack: u32, bp: u32, item: usize, count: u32 },
    Construct { bp: u32 },
    Wander { dest: TilePos },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Job {
    pub kind: JobKind,
    pub stage: u8,
    pub timer: f32,
    pub label: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Pawn {
    pub id: u32,
    pub name: String,
    pub creature: usize,
    /// Colours for the sprite's `$slots`.
    pub slots: BTreeMap<String, String>,
    pub pos: (f32, f32),
    /// Reversed: next step is `last()`.
    pub path: Vec<TilePos>,
    pub job: Option<Job>,
    pub carrying: Option<(usize, u32)>,
    pub think: u32,
    /// Particles gathered toward the next item, per item.
    pub gathered: BTreeMap<usize, u32>,
    pub drafted: bool,
}

impl Pawn {
    pub fn tile(&self) -> TilePos {
        tile_of_f(self.pos.0, self.pos.1)
    }
    pub fn activity(&self) -> &str {
        self.job.as_ref().map(|j| j.label.as_str()).unwrap_or("Idle")
    }
}

/// Things the game turns into script hooks.
#[derive(Clone, Debug)]
pub enum ColonyEvent {
    Destroyed { def: usize, center: (i32, i32) },
    Harvested { def: usize, center: (i32, i32) },
    Built { def: usize, center: (i32, i32), id: ObjId },
    Message(String),
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Colony {
    pub pawns: Vec<Pawn>,
    pub next_pawn: u32,
    pub items: Items,
    pub designations: BTreeMap<TilePos, Designation>,
    pub blueprints: BTreeMap<u32, Blueprint>,
    pub next_bp: u32,
    pub stockpile: BTreeSet<TilePos>,
}

enum Step {
    Continue,
    Done,
    Fail(&'static str),
}

struct Ctx<'a> {
    world: &'a mut World,
    content: &'a Content,
    rng: &'a mut Rng,
    events: &'a mut Vec<ColonyEvent>,
}

fn tags_match(target_tags: &[String], tags: &[String]) -> bool {
    target_tags.is_empty() || target_tags.iter().any(|t| tags.contains(t))
}

fn near(t: TilePos, blocks_center: bool) -> impl Fn(TilePos) -> bool + Copy {
    move |p: TilePos| p.chebyshev(t) <= 1 && !(blocks_center && p == t)
}

impl Colony {
    pub fn after_load(&mut self) {
        self.items.rebuild_index();
    }

    pub fn spawn_pawn(&mut self, content: &Content, creature: usize, tile: TilePos, rng: &mut Rng) -> u32 {
        let def = content.creatures.by_idx(creature);
        self.next_pawn += 1;
        let name = if def.names.is_empty() {
            format!("Colonist {}", self.next_pawn)
        } else {
            def.names[rng.below(def.names.len())].clone()
        };
        let slots = def
            .slots
            .iter()
            .map(|(k, v)| (k.clone(), if v.is_empty() { "#ff00ff".into() } else { v[rng.below(v.len())].clone() }))
            .collect();
        let (x, y) = tile.center();
        self.pawns.push(Pawn {
            id: self.next_pawn,
            name,
            creature,
            slots,
            pos: (x, y),
            think: rng.below(20) as u32,
            ..Default::default()
        });
        self.next_pawn
    }

    // ------------------------------------------------------------ player orders

    /// Mark work in a rectangle. Returns how many designations were added.
    pub fn designate(&mut self, world: &World, content: &Content, job: usize, rect: TileRect, plant: Option<usize>) -> usize {
        let jd = content.jobs.by_idx(job);
        let mut added = 0;
        match jd.driver {
            JobDriver::Fell | JobDriver::Harvest | JobDriver::Deconstruct => {
                let mut seen = BTreeSet::new();
                for t in rect.iter() {
                    for o in world.objects_in_tile(t) {
                        if !seen.insert(o) {
                            continue;
                        }
                        let Some(inst) = world.objects.get(o) else { continue };
                        let proto = &content.protos[inst.def];
                        let ok = match jd.driver {
                            JobDriver::Deconstruct => inst.built,
                            JobDriver::Harvest => tags_match(&jd.targets, &proto.tags) && objs::has_yield(world, content, o),
                            _ => !inst.built && tags_match(&jd.targets, &proto.tags),
                        };
                        if !ok {
                            continue;
                        }
                        let key = inst.origin_tile();
                        if let std::collections::btree_map::Entry::Vacant(e) = self.designations.entry(key) {
                            e.insert(Designation { job, target: DesTarget::Object(o), reserved: None });
                            added += 1;
                        }
                    }
                }
            }
            JobDriver::Mine => {
                for t in rect.iter() {
                    if self.designations.contains_key(&t) {
                        continue;
                    }
                    if mineable_cell(world, content, t, &jd.targets).is_some() {
                        self.designations.insert(t, Designation { job, target: DesTarget::Tile, reserved: None });
                        added += 1;
                    }
                }
            }
            JobDriver::Plant => {
                let Some(def) = plant else { return 0 };
                for t in rect.iter() {
                    if self.designations.contains_key(&t) || !world.walkable(t) {
                        continue;
                    }
                    let soil = World::tile_cells(t).all(|(x, y)| {
                        world.get(x, y, Layer::Solid) == NONE_MAT
                            && tags_match(&jd.targets, &content.mat_rt(world.get(x, y, Layer::Floor)).tags)
                    });
                    if soil {
                        self.designations.insert(t, Designation { job, target: DesTarget::Plant(def), reserved: None });
                        added += 1;
                    }
                }
            }
            JobDriver::Haul | JobDriver::Construct => {}
        }
        added
    }

    /// Remove designations and blueprints in a rectangle (blueprint materials are dropped back).
    pub fn cancel(&mut self, world: &mut World, content: &Content, rect: TileRect) {
        self.designations.retain(|t, _| !rect.contains(*t));
        let gone: Vec<u32> = self.blueprints.values().filter(|b| rect.contains(b.tile)).map(|b| b.id).collect();
        for id in gone {
            self.remove_blueprint(world, content, id, true);
        }
    }

    fn remove_blueprint(&mut self, world: &mut World, content: &Content, id: u32, refund: bool) {
        let Some(bp) = self.blueprints.remove(&id) else { return };
        if refund {
            if let Some(b) = &content.protos[bp.def].build {
                for (i, &(item, _)) in b.cost.iter().enumerate() {
                    if bp.delivered[i] > 0 {
                        self.items.add(world, content, item, bp.delivered[i], bp.tile);
                    }
                }
            }
            if let Some(o) = bp.obj {
                objs::destroy(world, content, o);
            }
        }
    }

    pub fn set_stockpile(&mut self, world: &World, rect: TileRect, on: bool) {
        for t in rect.iter() {
            if on {
                if world.walkable(t) {
                    self.stockpile.insert(t);
                }
            } else {
                self.stockpile.remove(&t);
            }
        }
    }

    /// Queue a building. Returns false if it cannot go there.
    pub fn place_blueprint(&mut self, world: &World, content: &Content, def: usize, tile: TilePos, seed: u64) -> bool {
        let proto = &content.protos[def];
        let Some(build) = &proto.build else { return false };
        if self.blueprints.values().any(|b| b.tile == tile) {
            return false;
        }
        let origin = match proto.anchor {
            Anchor::Tile => tile.origin(),
            Anchor::Center => {
                let (x, y) = tile.center();
                (x as i32, y as i32)
            }
        };
        if !objs::can_place(world, content, def, origin, seed) {
            return false;
        }
        // Same layer already taken by something built here?
        let cells = proto.cells(seed);
        let clash = cells.iter().any(|c| {
            let (x, y) = (origin.0 + c.dx, origin.1 + c.dy);
            c.layer != Layer::Floor && world.get(x, y, c.layer) != NONE_MAT
        });
        if clash {
            return false;
        }
        self.next_bp += 1;
        let n = build.cost.len();
        self.blueprints.insert(
            self.next_bp,
            Blueprint {
                id: self.next_bp,
                def,
                tile,
                origin,
                seed,
                delivered: vec![0; n],
                incoming: vec![0; n],
                progress: 0,
                placed: 0,
                obj: None,
                reserved: None,
            },
        );
        true
    }

    // ------------------------------------------------------------ simulation

    pub fn tick(&mut self, world: &mut World, content: &Content, rng: &mut Rng, tick: u64) -> Vec<ColonyEvent> {
        let mut events = Vec::new();
        if tick % 30 == 0 {
            self.cleanup(world, content);
        }
        for i in 0..self.pawns.len() {
            let mut pawn = std::mem::take(&mut self.pawns[i]);
            let mut ctx = Ctx { world, content, rng, events: &mut events };
            self.tick_pawn(&mut pawn, &mut ctx);
            self.pawns[i] = pawn;
        }
        events
    }

    /// Drop stale orders and move items off tiles that became solid.
    fn cleanup(&mut self, world: &mut World, content: &Content) {
        self.designations.retain(|t, d| match &d.target {
            DesTarget::Object(o) => world.objects.get(*o).is_some(),
            DesTarget::Tile => {
                mineable_cell(world, content, *t, &content.jobs.by_idx(d.job).targets).is_some() || !world.is_loaded(t.origin().0, t.origin().1)
            }
            DesTarget::Plant(_) => true,
        });
        let stuck: Vec<(u32, usize, u32, TilePos)> = self
            .items
            .stacks
            .values()
            .filter(|s| world.chunk(s.tile.chunk()).is_some() && !world.walkable(s.tile) && s.reserved.is_none())
            .map(|s| (s.id, s.item, s.count, s.tile))
            .collect();
        for (id, item, count, tile) in stuck {
            self.items.take(id, count);
            self.items.add(world, content, item, count, tile);
        }
        self.stockpile.retain(|t| world.chunk(t.chunk()).is_none() || world.walkable(*t));
    }

    fn tick_pawn(&mut self, pawn: &mut Pawn, ctx: &mut Ctx) {
        // Pushed into a wall (something got built on us)? Step out.
        let here = pawn.tile();
        if ctx.world.chunk(here.chunk()).is_some() && !ctx.world.walkable(here) {
            if let Some(t) = nearest_tile(here, 4, |t| ctx.world.walkable(t)) {
                let (x, y) = t.center();
                pawn.pos = (x, y);
                pawn.path.clear();
            }
        }

        if pawn.job.is_none() {
            if pawn.think > 0 {
                pawn.think -= 1;
                return;
            }
            pawn.think = 15;
            pawn.job = self.find_job(pawn, ctx);
            if pawn.job.is_none() {
                self.drop_carried(pawn, ctx);
                if ctx.rng.chance(0.08) {
                    let start = pawn.tile();
                    let dx = ctx.rng.range_i32(-5, 5);
                    let dy = ctx.rng.range_i32(-5, 5);
                    let dest = start.offset(dx, dy);
                    if ctx.world.walkable(dest) {
                        pawn.job = Some(Job { kind: JobKind::Wander { dest }, stage: 0, timer: 0.0, label: "Wandering".into() });
                    }
                }
                return;
            }
        }
        let step = self.run_job(pawn, ctx);
        match step {
            Step::Continue => {}
            Step::Done => self.end_job(pawn, ctx, true),
            Step::Fail(_why) => self.end_job(pawn, ctx, false),
        }
    }

    fn end_job(&mut self, pawn: &mut Pawn, ctx: &mut Ctx, _ok: bool) {
        if let Some(job) = pawn.job.take() {
            match job.kind {
                JobKind::Work { at, .. } => {
                    if let Some(d) = self.designations.get_mut(&at) {
                        if d.reserved == Some(pawn.id) {
                            d.reserved = None;
                        }
                    }
                }
                JobKind::HaulToStockpile { stack, .. } => self.unreserve_stack(stack, pawn.id),
                JobKind::HaulToBlueprint { stack, bp, item, count } => {
                    self.unreserve_stack(stack, pawn.id);
                    if job.stage < 3 {
                        if let Some(b) = self.blueprints.get_mut(&bp) {
                            if let Some(i) = ctx.content.protos[b.def].build.as_ref().and_then(|bd| bd.cost.iter().position(|c| c.0 == item)) {
                                b.incoming[i] = b.incoming[i].saturating_sub(count);
                            }
                        }
                    }
                }
                JobKind::Construct { bp } => {
                    if let Some(b) = self.blueprints.get_mut(&bp) {
                        if b.reserved == Some(pawn.id) {
                            b.reserved = None;
                        }
                    }
                }
                JobKind::Wander { .. } => {}
            }
        }
        pawn.path.clear();
        self.drop_carried(pawn, ctx);
        pawn.think = 0;
    }

    fn unreserve_stack(&mut self, id: u32, pawn: u32) {
        if let Some(s) = self.items.stacks.get_mut(&id) {
            if s.reserved == Some(pawn) {
                s.reserved = None;
            }
        }
    }

    fn drop_carried(&mut self, pawn: &mut Pawn, ctx: &mut Ctx) {
        if let Some((item, n)) = pawn.carrying.take() {
            self.items.add(ctx.world, ctx.content, item, n, pawn.tile());
        }
    }

    /// Move one tick along the path. Returns true when the path is finished.
    fn walk(pawn: &mut Pawn, ctx: &mut Ctx) -> Result<bool, ()> {
        let Some(&next) = pawn.path.last() else { return Ok(true) };
        let Some(info) = ctx.world.tile(next) else { return Err(()) };
        if !info.walkable {
            return Err(());
        }
        let speed = ctx.content.creatures.by_idx(pawn.creature).speed;
        let mut budget = speed * TILE as f32 / TPS / info.cost.max(0.2);
        while budget > 0.0 {
            let Some(&next) = pawn.path.last() else { break };
            let (tx, ty) = next.center();
            let (dx, dy) = (tx - pawn.pos.0, ty - pawn.pos.1);
            let d = (dx * dx + dy * dy).sqrt();
            if d <= budget {
                pawn.pos = (tx, ty);
                budget -= d;
                pawn.path.pop();
            } else {
                pawn.pos.0 += dx / d * budget;
                pawn.pos.1 += dy / d * budget;
                budget = 0.0;
            }
        }
        Ok(pawn.path.is_empty())
    }

    fn path_to(pawn: &mut Pawn, ctx: &Ctx, goal: impl Fn(TilePos) -> bool, hint: TilePos) -> bool {
        let w = &*ctx.world;
        match find_path(w, pawn.tile(), |t| goal(t) && w.walkable(t), hint, MAX_PATH_NODES) {
            Some(p) => {
                pawn.path = p;
                true
            }
            None => false,
        }
    }

    // ------------------------------------------------------------ job search

    fn find_job(&mut self, pawn: &mut Pawn, ctx: &mut Ctx) -> Option<Job> {
        if pawn.drafted {
            return None;
        }
        let here = pawn.tile();
        for ji in ctx.content.jobs_by_priority() {
            let jd = ctx.content.jobs.by_idx(ji);
            let found = match jd.driver {
                JobDriver::Haul => self.find_haul(pawn, ctx, here),
                JobDriver::Construct => self.find_construct(pawn, ctx, here),
                _ => self.find_designation(pawn, ctx, ji, here),
            };
            if found.is_some() {
                return found;
            }
        }
        None
    }

    fn find_designation(&mut self, pawn: &mut Pawn, ctx: &mut Ctx, job: usize, here: TilePos) -> Option<Job> {
        let jd = ctx.content.jobs.by_idx(job);
        let mut cands: Vec<(i32, TilePos)> = self
            .designations
            .iter()
            .filter(|(_, d)| d.job == job && d.reserved.is_none())
            .map(|(t, _)| (t.manhattan(here), *t))
            .collect();
        cands.sort();
        for (_, t) in cands.into_iter().take(6) {
            let blocks = match &self.designations[&t].target {
                DesTarget::Object(o) => ctx.world.objects.get(*o).is_some_and(|i| ctx.content.protos[i.def].blocks),
                DesTarget::Tile => true,
                DesTarget::Plant(_) => false,
            };
            if Self::path_to(pawn, ctx, near(t, blocks), t) {
                self.designations.get_mut(&t).unwrap().reserved = Some(pawn.id);
                return Some(Job { kind: JobKind::Work { at: t, driver: jd.driver }, stage: 0, timer: 0.0, label: jd.label.clone() });
            }
        }
        None
    }

    fn find_construct(&mut self, pawn: &mut Pawn, ctx: &mut Ctx, here: TilePos) -> Option<Job> {
        let mut cands: Vec<(i32, u32)> = self
            .blueprints
            .values()
            .filter(|b| b.reserved.is_none() && b.ready(ctx.content))
            .map(|b| (b.tile.manhattan(here), b.id))
            .collect();
        cands.sort();
        for (_, id) in cands.into_iter().take(6) {
            let (tile, blocks) = {
                let b = &self.blueprints[&id];
                (b.tile, ctx.content.protos[b.def].blocks)
            };
            if Self::path_to(pawn, ctx, near(tile, blocks), tile) {
                self.blueprints.get_mut(&id).unwrap().reserved = Some(pawn.id);
                let name = ctx.content.objects.by_idx(self.blueprints[&id].def).name.clone();
                return Some(Job { kind: JobKind::Construct { bp: id }, stage: 0, timer: 0.0, label: format!("Building {name}") });
            }
        }
        None
    }

    fn nearest_stack(&self, here: TilePos, item: Option<usize>, skip_stockpiled: bool) -> Vec<u32> {
        let mut v: Vec<(i32, u32)> = self
            .items
            .stacks
            .values()
            .filter(|s| s.reserved.is_none() && item.is_none_or(|i| s.item == i))
            .filter(|s| !skip_stockpiled || !self.stockpile.contains(&s.tile))
            .map(|s| (s.tile.manhattan(here), s.id))
            .collect();
        v.sort();
        v.into_iter().map(|x| x.1).take(6).collect()
    }

    fn find_haul(&mut self, pawn: &mut Pawn, ctx: &mut Ctx, here: TilePos) -> Option<Job> {
        // 1. Materials for blueprints.
        let mut bps: Vec<(i32, u32)> = self.blueprints.values().map(|b| (b.tile.manhattan(here), b.id)).collect();
        bps.sort();
        for (_, bp) in bps.into_iter().take(8) {
            let needs = self.blueprints[&bp].needs(ctx.content);
            for (item, need) in needs {
                for sid in self.nearest_stack(here, Some(item), false) {
                    let tile = self.items.stacks[&sid].tile;
                    if Self::path_to(pawn, ctx, move |t| t == tile, tile) {
                        let count = need.min(self.items.stacks[&sid].count);
                        self.items.stacks.get_mut(&sid).unwrap().reserved = Some(pawn.id);
                        let b = self.blueprints.get_mut(&bp).unwrap();
                        let ci = ctx.content.protos[b.def].build.as_ref().unwrap().cost.iter().position(|c| c.0 == item).unwrap();
                        b.incoming[ci] += count;
                        let name = ctx.content.items.by_idx(item).name.clone();
                        return Some(Job {
                            kind: JobKind::HaulToBlueprint { stack: sid, bp, item, count },
                            stage: 0,
                            timer: 0.0,
                            label: format!("Hauling {name} to build"),
                        });
                    }
                }
            }
        }
        // 2. Loose items to stockpiles.
        if self.stockpile.is_empty() {
            return None;
        }
        for sid in self.nearest_stack(here, None, true) {
            let (item, tile) = {
                let s = &self.items.stacks[&sid];
                (s.item, s.tile)
            };
            let Some(dest) = self.free_stockpile_tile(ctx, item, tile) else { continue };
            if Self::path_to(pawn, ctx, move |t| t == tile, tile) {
                self.items.stacks.get_mut(&sid).unwrap().reserved = Some(pawn.id);
                let name = ctx.content.items.by_idx(item).name.clone();
                return Some(Job { kind: JobKind::HaulToStockpile { stack: sid, dest }, stage: 0, timer: 0.0, label: format!("Hauling {name}") });
            }
        }
        None
    }

    fn free_stockpile_tile(&self, ctx: &Ctx, item: usize, from: TilePos) -> Option<TilePos> {
        let max = ctx.content.item_rt[item].stack;
        let taken: BTreeSet<TilePos> = self
            .pawns
            .iter()
            .filter_map(|p| match &p.job {
                Some(Job { kind: JobKind::HaulToStockpile { dest, .. }, .. }) => Some(*dest),
                _ => None,
            })
            .collect();
        let mut best: Option<(i32, TilePos)> = None;
        for &t in &self.stockpile {
            if taken.contains(&t) || !ctx.world.walkable(t) {
                continue;
            }
            let ok = match self.items.at(t) {
                None => true,
                Some(s) => s.item == item && s.count < max,
            };
            if ok {
                let d = t.manhattan(from);
                // Prefer topping up existing stacks.
                let d = if self.items.at(t).is_some() { d - 1000 } else { d };
                if best.is_none_or(|(bd, _)| d < bd) {
                    best = Some((d, t));
                }
            }
        }
        best.map(|b| b.1)
    }

    // ------------------------------------------------------------ job execution

    fn run_job(&mut self, pawn: &mut Pawn, ctx: &mut Ctx) -> Step {
        let Some(job) = pawn.job.clone() else { return Step::Done };
        let step = match &job.kind {
            JobKind::Wander { .. } => match Self::walk(pawn, ctx) {
                Ok(true) | Err(()) => {
                    if job.stage == 0 {
                        if let JobKind::Wander { dest } = job.kind {
                            if !Self::path_to(pawn, ctx, move |t| t == dest, dest) {
                                return Step::Done;
                            }
                        }
                        self.set_stage(pawn, 1);
                        Step::Continue
                    } else {
                        Step::Done
                    }
                }
                Ok(false) => Step::Continue,
            },
            JobKind::Work { at, driver } => self.run_work(pawn, ctx, *at, *driver, job.stage),
            JobKind::HaulToStockpile { stack, dest } => self.run_haul_stockpile(pawn, ctx, *stack, *dest, job.stage),
            JobKind::HaulToBlueprint { stack, bp, item, count } => {
                self.run_haul_bp(pawn, ctx, *stack, *bp, *item, *count, job.stage)
            }
            JobKind::Construct { bp } => self.run_construct(pawn, ctx, *bp, job.stage),
        };
        step
    }

    fn set_stage(&mut self, pawn: &mut Pawn, s: u8) {
        if let Some(j) = &mut pawn.job {
            j.stage = s;
            j.timer = 0.0;
        }
    }

    fn timer(pawn: &mut Pawn) -> &mut f32 {
        &mut pawn.job.as_mut().unwrap().timer
    }

    /// Walk the current path; repath with `goal` if it gets blocked.
    fn follow(pawn: &mut Pawn, ctx: &mut Ctx, goal: impl Fn(TilePos) -> bool + Copy, hint: TilePos) -> Result<bool, ()> {
        match Self::walk(pawn, ctx) {
            Ok(done) => {
                if done && !goal(pawn.tile()) {
                    if !Self::path_to(pawn, ctx, goal, hint) {
                        return Err(());
                    }
                    return Ok(pawn.path.is_empty());
                }
                Ok(done)
            }
            Err(()) => {
                if Self::path_to(pawn, ctx, goal, hint) { Ok(false) } else { Err(()) }
            }
        }
    }

    fn gather(&mut self, pawn: &mut Pawn, ctx: &mut Ctx, mat: MatId, drop_at: TilePos) {
        if let Some((item, per)) = ctx.content.mat_rt(mat).yields {
            let g = pawn.gathered.entry(item).or_insert(0);
            *g += 1;
            if *g >= per {
                *g -= per;
                self.items.add(ctx.world, ctx.content, item, 1, drop_at);
            }
        }
    }

    fn run_work(&mut self, pawn: &mut Pawn, ctx: &mut Ctx, at: TilePos, driver: JobDriver, stage: u8) -> Step {
        let Some(des) = self.designations.get(&at).cloned() else { return Step::Fail("order cancelled") };
        let jd = ctx.content.jobs.by_idx(des.job);
        let blocks = match &des.target {
            DesTarget::Object(o) => ctx.world.objects.get(*o).is_some_and(|i| ctx.content.protos[i.def].blocks),
            DesTarget::Tile => true,
            DesTarget::Plant(_) => false,
        };
        if stage == 0 {
            return match Self::follow(pawn, ctx, near(at, blocks), at) {
                Ok(true) => {
                    self.set_stage(pawn, 1);
                    Step::Continue
                }
                Ok(false) => Step::Continue,
                Err(()) => Step::Fail("no path"),
            };
        }
        let drop_tile = if ctx.world.walkable(at) { at } else { pawn.tile() };
        let speed = jd.speed.max(0.05);
        match (driver, &des.target) {
            (JobDriver::Fell | JobDriver::Deconstruct, DesTarget::Object(o)) => {
                let o = *o;
                let Some(inst) = ctx.world.objects.get(o).cloned() else {
                    self.designations.remove(&at);
                    return Step::Done;
                };
                *Self::timer(pawn) += 1.0;
                while pawn.job.as_ref().unwrap().timer >= speed {
                    *Self::timer(pawn) -= speed;
                    if objs::chip(ctx.world, ctx.content, o, ctx.rng, None).is_none() {
                        break;
                    }
                }
                let cur = ctx.world.objects.get(o).cloned().unwrap_or(inst.clone());
                let proto = &ctx.content.protos[cur.def];
                if cur.removed as f32 >= cur.total as f32 * proto.break_fraction || objs::live_cells(ctx.world, o).is_empty() {
                    let c = objs::center(&cur);
                    objs::destroy(ctx.world, ctx.content, o);
                    if driver == JobDriver::Deconstruct {
                        if let Some(b) = &proto.build {
                            for &(item, n) in &b.cost {
                                let back = n.div_ceil(2);
                                if back > 0 {
                                    self.items.add(ctx.world, ctx.content, item, back, drop_tile);
                                }
                            }
                        }
                    } else {
                        for &(item, lo, hi) in &proto.drops {
                            let n = ctx.rng.range_i32(lo as i32, hi as i32) as u32;
                            if n > 0 {
                                self.items.add(ctx.world, ctx.content, item, n, cur.origin_tile());
                            }
                        }
                    }
                    ctx.events.push(ColonyEvent::Destroyed { def: cur.def, center: c });
                    self.designations.remove(&at);
                    return Step::Done;
                }
                Step::Continue
            }
            (JobDriver::Harvest, DesTarget::Object(o)) => {
                let o = *o;
                let Some(inst) = ctx.world.objects.get(o).cloned() else {
                    self.designations.remove(&at);
                    return Step::Done;
                };
                *Self::timer(pawn) += 1.0;
                if pawn.job.as_ref().unwrap().timer >= speed {
                    *Self::timer(pawn) = 0.0;
                    match objs::pluck(ctx.world, ctx.content, o, ctx.rng) {
                        Some((_, _, m)) => self.gather(pawn, ctx, m, drop_tile),
                        None => {
                            // Round up what is left over.
                            let left: Vec<(usize, u32)> = pawn.gathered.iter().filter(|(_, n)| **n > 0).map(|(i, n)| (*i, *n)).collect();
                            for (item, _) in left {
                                self.items.add(ctx.world, ctx.content, item, 1, drop_tile);
                                pawn.gathered.insert(item, 0);
                            }
                            ctx.events.push(ColonyEvent::Harvested { def: inst.def, center: objs::center(&inst) });
                            self.designations.remove(&at);
                            return Step::Done;
                        }
                    }
                }
                Step::Continue
            }
            (JobDriver::Mine, DesTarget::Tile) => {
                let Some((x, y)) = mineable_cell(ctx.world, ctx.content, at, &jd.targets) else {
                    self.designations.remove(&at);
                    return Step::Done;
                };
                let m = ctx.world.get(x, y, Layer::Solid);
                let need = speed * ctx.content.mat_rt(m).hardness;
                *Self::timer(pawn) += 1.0;
                if pawn.job.as_ref().unwrap().timer >= need {
                    *Self::timer(pawn) = 0.0;
                    // Take a small cluster so mining reads as digging.
                    let cells: Vec<(i32, i32)> = World::tile_cells(at)
                        .filter(|&(cx, cy)| (cx - x).abs() <= 1 && (cy - y).abs() <= 1)
                        .filter(|&(cx, cy)| mine_ok(ctx.world, ctx.content, cx, cy, &jd.targets))
                        .take(2)
                        .collect();
                    for (cx, cy) in cells {
                        let mm = ctx.world.get(cx, cy, Layer::Solid);
                        ctx.world.set(ctx.content, cx, cy, Layer::Solid, NONE_MAT, NO_OBJ);
                        self.gather(pawn, ctx, mm, pawn.tile());
                    }
                }
                Step::Continue
            }
            (JobDriver::Plant, DesTarget::Plant(def)) => {
                *Self::timer(pawn) += 1.0;
                if pawn.job.as_ref().unwrap().timer >= 45.0 * speed {
                    let def = *def;
                    let origin = match ctx.content.protos[def].anchor {
                        Anchor::Tile => at.origin(),
                        Anchor::Center => {
                            let (x, y) = at.center();
                            (x as i32, y as i32)
                        }
                    };
                    let seed = ctx.rng.next_u64();
                    if objs::can_place(ctx.world, ctx.content, def, origin, seed) {
                        objs::place(ctx.world, ctx.content, def, origin, seed, false);
                    }
                    self.designations.remove(&at);
                    return Step::Done;
                }
                Step::Continue
            }
            _ => {
                self.designations.remove(&at);
                Step::Fail("bad order")
            }
        }
    }

    fn run_haul_stockpile(&mut self, pawn: &mut Pawn, ctx: &mut Ctx, stack: u32, dest: TilePos, stage: u8) -> Step {
        match stage {
            0 => {
                let Some(s) = self.items.get(stack) else { return Step::Fail("items gone") };
                let tile = s.tile;
                match Self::follow(pawn, ctx, move |t| t == tile, tile) {
                    Ok(true) => {
                        let n = self.items.get(stack).map(|s| s.count).unwrap_or(0);
                        let item = self.items.get(stack).map(|s| s.item).unwrap_or(0);
                        let max = ctx.content.item_rt[item].stack;
                        let room = match self.items.at(dest) {
                            Some(d) if d.item == item => max.saturating_sub(d.count),
                            Some(_) => 0,
                            None => max,
                        };
                        let got = self.items.take(stack, n.min(room.max(1)));
                        if got == 0 {
                            return Step::Fail("nothing to carry");
                        }
                        self.unreserve_stack(stack, pawn.id);
                        pawn.carrying = Some((item, got));
                        if !Self::path_to(pawn, ctx, move |t| t == dest, dest) {
                            return Step::Fail("no path to stockpile");
                        }
                        self.set_stage(pawn, 1);
                        Step::Continue
                    }
                    Ok(false) => Step::Continue,
                    Err(()) => Step::Fail("no path"),
                }
            }
            _ => match Self::follow(pawn, ctx, move |t| t == dest, dest) {
                Ok(true) => {
                    if let Some((item, n)) = pawn.carrying.take() {
                        self.items.add(ctx.world, ctx.content, item, n, dest);
                    }
                    Step::Done
                }
                Ok(false) => Step::Continue,
                Err(()) => Step::Fail("no path"),
            },
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn run_haul_bp(&mut self, pawn: &mut Pawn, ctx: &mut Ctx, stack: u32, bp: u32, item: usize, count: u32, stage: u8) -> Step {
        let Some(b) = self.blueprints.get(&bp) else { return Step::Fail("blueprint gone") };
        let (bt, blocks) = (b.tile, ctx.content.protos[b.def].blocks);
        match stage {
            0 => {
                let Some(s) = self.items.get(stack) else { return Step::Fail("items gone") };
                let tile = s.tile;
                match Self::follow(pawn, ctx, move |t| t == tile, tile) {
                    Ok(true) => {
                        let got = self.items.take(stack, count);
                        self.unreserve_stack(stack, pawn.id);
                        if got == 0 {
                            return Step::Fail("nothing to carry");
                        }
                        pawn.carrying = Some((item, got));
                        if !Self::path_to(pawn, ctx, near(bt, blocks), bt) {
                            return Step::Fail("no path to blueprint");
                        }
                        self.set_stage(pawn, 1);
                        Step::Continue
                    }
                    Ok(false) => Step::Continue,
                    Err(()) => Step::Fail("no path"),
                }
            }
            _ => match Self::follow(pawn, ctx, near(bt, blocks), bt) {
                Ok(true) => {
                    let b = self.blueprints.get_mut(&bp).unwrap();
                    let ci = ctx.content.protos[b.def].build.as_ref().unwrap().cost.iter().position(|c| c.0 == item).unwrap();
                    let (it, n) = pawn.carrying.take().unwrap_or((item, 0));
                    b.delivered[ci] += n;
                    b.incoming[ci] = b.incoming[ci].saturating_sub(count);
                    let _ = it;
                    self.set_stage(pawn, 3);
                    Step::Done
                }
                Ok(false) => Step::Continue,
                Err(()) => Step::Fail("no path"),
            },
        }
    }

    fn run_construct(&mut self, pawn: &mut Pawn, ctx: &mut Ctx, bp: u32, stage: u8) -> Step {
        let Some(b) = self.blueprints.get(&bp) else { return Step::Fail("blueprint gone") };
        let (bt, def) = (b.tile, b.def);
        let proto = &ctx.content.protos[def];
        if stage == 0 {
            return match Self::follow(pawn, ctx, near(bt, proto.blocks), bt) {
                Ok(true) => {
                    self.set_stage(pawn, 1);
                    Step::Continue
                }
                Ok(false) => Step::Continue,
                Err(()) => Step::Fail("no path"),
            };
        }
        let work = proto.build.as_ref().map(|b| b.work).unwrap_or(1);
        let b = self.blueprints.get_mut(&bp).unwrap();
        let mut cells = proto.cells(b.seed);
        cells.sort_by_key(|c| (c.layer, c.dy, c.dx));
        if b.obj.is_none() {
            b.obj = Some(objs::create_instance(ctx.world, ctx.content, def, b.origin, b.seed, true));
        }
        let id = b.obj.unwrap();
        b.progress += 1;
        let target = ((b.progress as usize * cells.len()) / work as usize).min(cells.len());
        while b.placed < target {
            let c = cells[b.placed];
            let (x, y) = (b.origin.0 + c.dx, b.origin.1 + c.dy);
            ctx.world.set_raw(ctx.content, x, y, c.layer, c.mat, id);
            b.placed += 1;
        }
        if b.progress >= work {
            let center = ctx.world.objects.get(id).map(objs::center).unwrap_or(b.origin);
            self.blueprints.remove(&bp);
            ctx.events.push(ColonyEvent::Built { def, center, id });
            return Step::Done;
        }
        Step::Continue
    }
}

fn mine_ok(world: &World, content: &Content, x: i32, y: i32, targets: &[String]) -> bool {
    let m = world.get(x, y, Layer::Solid);
    m != NONE_MAT && world.get_obj(x, y, Layer::Solid) == NO_OBJ && tags_match(targets, &content.mat_rt(m).tags)
}

/// A minable particle in this tile, if any.
pub fn mineable_cell(world: &World, content: &Content, t: TilePos, targets: &[String]) -> Option<(i32, i32)> {
    World::tile_cells(t).find(|&(x, y)| mine_ok(world, content, x, y, targets))
}
