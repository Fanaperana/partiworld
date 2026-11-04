//! Colonists and their work: designations -> jobs -> stages.
//! Job kinds are data (`JobDef`) pointing at built-in drivers, so a mod can add
//! e.g. "cut flowers" as a Fell job targeting the `flower` tag.

pub mod path;

use std::collections::{BTreeMap, BTreeSet, HashMap};

use pw_core::*;
use pw_objects as objs;
use pw_world::*;
use serde::{Deserialize, Serialize};

pub use path::{find_nearest, find_path, nearest_tile};

/// Simulation ticks per second at 1x speed.
pub const TPS: f32 = 30.0;
const MAX_PATH_NODES: usize = 6000;
/// Nodes for the "nearest of many targets" search before falling back to A*.
const NEAR_NODES: usize = 2500;
/// Most blueprints one supply trip drops materials at.
const MAX_DROPS: usize = 12;
/// Work priority: 1 = do first, 4 = do last, 0 = never.
pub const DEFAULT_WORK: u8 = 3;
pub const MAX_WORK: u8 = 4;

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
    pub fn add(
        &mut self,
        world: &World,
        content: &Content,
        item: usize,
        mut count: u32,
        near: TilePos,
    ) {
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
                    let add = if found.is_some() {
                        (max - s.count.min(max)).min(count)
                    } else {
                        count
                    };
                    s.count += add;
                    count -= add;
                    continue;
                }
            }
            self.next += 1;
            let n = count.min(max);
            let id = self.next;
            self.stacks.insert(
                id,
                ItemStack {
                    id,
                    item,
                    count: n,
                    tile: t,
                    reserved: None,
                },
            );
            self.by_tile.insert(t, id);
            count -= n;
        }
    }

    /// Take up to `n` from a stack. Returns how many were taken.
    pub fn take(&mut self, id: u32, n: u32) -> u32 {
        let Some(s) = self.stacks.get_mut(&id) else {
            return 0;
        };
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
        self.stacks
            .values()
            .filter(|s| s.item == item)
            .map(|s| s.count)
            .sum()
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
        let Some(b) = &content.protos[self.def].build else {
            return Vec::new();
        };
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
        let Some(b) = &content.protos[self.def].build else {
            return true;
        };
        b.cost
            .iter()
            .enumerate()
            .all(|(i, &(_, n))| self.delivered[i] >= n)
    }
}

// ---------------------------------------------------------------- pawns

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum JobKind {
    /// Work a designation (fell, mine, harvest, deconstruct, plant).
    Work {
        at: TilePos,
        driver: JobDriver,
    },
    HaulToStockpile {
        stack: u32,
        dest: TilePos,
    },
    /// Pick up one stack and drop it off at several blueprints (stage 1 + i = going to drop i).
    Supply {
        stack: u32,
        item: usize,
        drops: Vec<(u32, u32)>,
    },
    /// Put what the pawn already carries in the stockpile.
    Store {
        dest: TilePos,
    },
    Construct {
        bp: u32,
    },
    Wander {
        dest: TilePos,
    },
    /// Walk to a food stack and eat from it.
    Eat {
        stack: u32,
    },
    /// Sleep at `at`, in `bed` if there is one.
    Sleep {
        bed: Option<ObjId>,
        at: TilePos,
    },
    /// Stand somewhere warm until warmed up.
    Warm {
        at: TilePos,
    },
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
    /// Work priority per job id (1 = first ... 4 = last, 0 = never). Missing = DEFAULT_WORK.
    #[serde(default)]
    pub work: BTreeMap<String, u8>,
    #[serde(default)]
    pub needs: Needs,
}

/// How a colonist is doing. Each value is 0..1: 1 = fully met, 0 = in danger.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Needs {
    pub food: f32,
    pub rest: f32,
    pub warmth: f32,
    pub health: f32,
    /// Temperature where the colonist stands, C (updated now and then).
    pub felt: f32,
    /// Don't look for a warm spot again before this tick (none was found).
    pub warm_retry: u64,
}

impl Default for Needs {
    fn default() -> Self {
        Self {
            food: 1.0,
            rest: 1.0,
            warmth: 1.0,
            health: 1.0,
            felt: 15.0,
            warm_retry: 0,
        }
    }
}

const WARN_FOOD: usize = 0;
const WARN_COLD: usize = 1;

impl Needs {
    /// The most pressing problem, for labels.
    pub fn worst(&self) -> Option<&'static str> {
        if self.food <= 0.0 {
            Some("starving")
        } else if self.warmth <= 0.0 {
            Some("freezing")
        } else if self.health < 0.5 {
            Some("hurt")
        } else if self.food < 0.3 {
            Some("hungry")
        } else if self.warmth < 0.4 {
            Some("cold")
        } else if self.rest < 0.25 {
            Some("tired")
        } else {
            None
        }
    }
}

/// What needs depend on, from the world around the colony.
#[derive(Clone, Debug, Default)]
pub struct NeedsEnv {
    pub tick: u64,
    pub day_ticks: u64,
    /// 0 = day, 1 = night.
    pub dark: f32,
    /// Outdoor temperature, C.
    pub temp: f32,
    pub wind: f32,
    pub wet: f32,
    /// Other peoples' land (centre, radius in tiles): their beds are not ours.
    pub keep_out: Vec<(TilePos, i32)>,
}

/// Ticks between needs updates.
const NEEDS_EVERY: u64 = 30;
/// Tiles around a spot that can warm it.
const HEAT_RANGE: i32 = 4;

/// Shelter from wind and rain: a roof over the tile.
pub fn roofed(world: &World, content: &Content, t: TilePos) -> bool {
    let (x, y) = t.origin();
    content
        .mat_rt(world.get(x + TILE / 2, y + TILE / 2, Layer::Overlay))
        .roof
}

/// Heat given off by one tile's particles (every other particle is sampled).
fn tile_heat(world: &World, content: &Content, t: TilePos) -> f32 {
    if world.chunk(t.chunk()).is_none() {
        return 0.0;
    }
    let (x0, y0) = t.origin();
    let mut h = 0.0;
    for py in (0..TILE).step_by(2) {
        for px in (0..TILE).step_by(2) {
            for l in [Layer::Solid, Layer::Floor] {
                h += content.mat_rt(world.get(x0 + px, y0 + py, l)).heat;
            }
        }
    }
    h
}

/// How much a tile's heat warms a spot `d` tiles away.
fn heat_falloff(h: f32, d: i32) -> f32 {
    h * 2.5 / (1.0 + d as f32 * 0.5)
}

/// Warmth from fires, embers and lava near a tile, C.
pub fn heat_near(world: &World, content: &Content, t: TilePos) -> f32 {
    let mut total = 0.0;
    for dy in -HEAT_RANGE..=HEAT_RANGE {
        for dx in -HEAT_RANGE..=HEAT_RANGE {
            let h = tile_heat(world, content, t.offset(dx, dy));
            if h > 0.0 {
                total += heat_falloff(h, dx.abs().max(dy.abs()));
            }
        }
    }
    total.min(40.0)
}

/// The weather where `t` is: softened under a roof, colder in wind and rain.
fn sheltered_temp(world: &World, content: &Content, t: TilePos, env: &NeedsEnv) -> f32 {
    let mut temp = env.temp;
    if roofed(world, content, t) {
        if temp < 18.0 {
            temp += (18.0 - temp) * 0.4;
        }
    } else {
        temp -= env.wind * 4.0 + env.wet * 3.0;
    }
    temp
}

/// The temperature a colonist feels standing on `t`: the weather, softened
/// under a roof, plus heat from fires close by.
pub fn felt_temp(world: &World, content: &Content, t: TilePos, env: &NeedsEnv) -> f32 {
    sheltered_temp(world, content, t, env) + heat_near(world, content, t)
}

fn on_hot(world: &World, content: &Content, pos: (f32, f32)) -> bool {
    let (x, y) = (pos.0 as i32, pos.1 as i32);
    [Layer::Floor, Layer::Solid, Layer::Overlay]
        .iter()
        .any(|&l| content.mat_rt(world.get(x, y, l)).hot)
}

impl Pawn {
    pub fn work_priority(&self, job: &str) -> u8 {
        self.work.get(job).copied().unwrap_or(DEFAULT_WORK)
    }
    /// Cycle 1 -> 2 -> 3 -> 4 -> off -> 1.
    pub fn cycle_work(&mut self, job: &str) {
        let p = self.work_priority(job);
        let n = match p {
            0 => 1,
            p if p >= MAX_WORK => 0,
            p => p + 1,
        };
        self.work.insert(job.to_string(), n);
    }
    pub fn tile(&self) -> TilePos {
        tile_of_f(self.pos.0, self.pos.1)
    }
    pub fn activity(&self) -> &str {
        self.job
            .as_ref()
            .map(|j| j.label.as_str())
            .unwrap_or("Idle")
    }
    pub fn sleeping(&self) -> bool {
        matches!(
            &self.job,
            Some(Job {
                kind: JobKind::Sleep { .. },
                stage: 1,
                ..
            })
        )
    }
    /// Walking speed factor from exhaustion and wounds.
    pub fn pace(&self) -> f32 {
        let mut p = 1.0;
        if self.needs.rest < 0.15 {
            p *= 0.75;
        }
        if self.needs.health < 0.5 {
            p *= 0.8;
        }
        p
    }
}

/// Things the game turns into script hooks.
#[derive(Clone, Debug)]
pub enum ColonyEvent {
    Destroyed {
        def: usize,
        center: (i32, i32),
    },
    Harvested {
        def: usize,
        center: (i32, i32),
    },
    Built {
        def: usize,
        center: (i32, i32),
        id: ObjId,
    },
    Message(String),
    /// A colonist died of its needs (`cause`: "starved", "froze", "burned").
    Died {
        name: String,
        cause: &'static str,
        at: TilePos,
    },
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
    /// When each kind of needs warning was last given (so they aren't repeated).
    #[serde(default)]
    pub warned_at: [u64; 2],
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
    walk: f32,
}

fn tags_match(target_tags: &[String], tags: &[String]) -> bool {
    target_tags.is_empty() || target_tags.iter().any(|t| tags.contains(t))
}

fn near(t: TilePos, blocks_center: bool) -> impl Fn(TilePos) -> bool + Copy {
    move |p: TilePos| p.chebyshev(t) <= 1 && !(blocks_center && p == t)
}

/// Tiles a pawn can stand on to work at `t`.
fn stand_tiles(t: TilePos, blocks_center: bool) -> impl Iterator<Item = TilePos> {
    (-1..=1)
        .flat_map(move |dy| (-1..=1).map(move |dx| t.offset(dx, dy)))
        .filter(move |&p| !(blocks_center && p == t))
}

/// What a construct search can land on.
#[derive(Clone, Copy)]
enum BuildCand {
    Build(u32),
    Stack(u32),
}

/// True if a pawn at `start` is not boxed in by walls plus `blocked` tiles
/// (unbuilt walls). Used so builders stand outside the room they are closing.
fn open_area(world: &World, blocked: &BTreeSet<TilePos>, start: TilePos, limit: usize) -> bool {
    let mut seen = BTreeSet::from([start]);
    let mut queue = std::collections::VecDeque::from([start]);
    while let Some(t) = queue.pop_front() {
        if seen.len() >= limit {
            return true;
        }
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let n = t.offset(dx, dy);
            if world.chunk(n.chunk()).is_none() {
                return true; // reaches unloaded land: open
            }
            if world.walkable(n) && !blocked.contains(&n) && seen.insert(n) {
                queue.push_back(n);
            }
        }
    }
    false
}

impl Colony {
    pub fn after_load(&mut self) {
        self.items.rebuild_index();
    }

    /// Take a pawn out of the colony (death, leaving): its job is let go and
    /// anything it carried is dropped where it stands.
    pub fn remove_pawn(
        &mut self,
        world: &mut World,
        content: &Content,
        rng: &mut Rng,
        id: u32,
    ) -> Option<Pawn> {
        let i = self.pawns.iter().position(|p| p.id == id)?;
        let mut pawn = self.pawns.remove(i);
        let mut events = Vec::new();
        let mut ctx = Ctx {
            world,
            content,
            rng,
            events: &mut events,
            walk: 1.0,
        };
        self.end_job(&mut pawn, &mut ctx, false);
        if let Some((item, n)) = pawn.carrying.take() {
            self.items.add(world, content, item, n, pawn.tile());
        }
        Some(pawn)
    }

    /// Add an existing pawn (moving between colonies). It gets a new id here.
    pub fn adopt_pawn(&mut self, mut pawn: Pawn) -> u32 {
        self.next_pawn += 1;
        pawn.id = self.next_pawn;
        pawn.job = None;
        pawn.path.clear();
        self.pawns.push(pawn);
        self.next_pawn
    }

    pub fn spawn_pawn(
        &mut self,
        content: &Content,
        creature: usize,
        tile: TilePos,
        rng: &mut Rng,
    ) -> u32 {
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
            .map(|(k, v)| {
                (
                    k.clone(),
                    if v.is_empty() {
                        "#ff00ff".into()
                    } else {
                        v[rng.below(v.len())].clone()
                    },
                )
            })
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
    pub fn designate(
        &mut self,
        world: &World,
        content: &Content,
        job: usize,
        rect: TileRect,
        plant: Option<usize>,
    ) -> usize {
        let jd = content.jobs.by_idx(job);
        let mut added = 0;
        match jd.driver {
            JobDriver::Fell | JobDriver::Harvest | JobDriver::Deconstruct => {
                for o in world.objects_in_rect(rect) {
                    let Some(inst) = world.objects.get(o) else {
                        continue;
                    };
                    let proto = &content.protos[inst.def];
                    let ok = match jd.driver {
                        JobDriver::Deconstruct => inst.built,
                        JobDriver::Harvest => {
                            tags_match(&jd.targets, &proto.tags)
                                && objs::has_yield(world, content, o)
                        }
                        _ => !inst.built && tags_match(&jd.targets, &proto.tags),
                    };
                    if !ok {
                        continue;
                    }
                    let key = inst.origin_tile();
                    if let std::collections::btree_map::Entry::Vacant(e) =
                        self.designations.entry(key)
                    {
                        e.insert(Designation {
                            job,
                            target: DesTarget::Object(o),
                            reserved: None,
                        });
                        added += 1;
                    }
                }
            }
            JobDriver::Mine => {
                for t in rect.iter() {
                    if self.designations.contains_key(&t) {
                        continue;
                    }
                    if mineable_cell(world, content, t, &jd.targets).is_some() {
                        self.designations.insert(
                            t,
                            Designation {
                                job,
                                target: DesTarget::Tile,
                                reserved: None,
                            },
                        );
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
                            && tags_match(
                                &jd.targets,
                                &content.mat_rt(world.get(x, y, Layer::Floor)).tags,
                            )
                    });
                    if soil {
                        self.designations.insert(
                            t,
                            Designation {
                                job,
                                target: DesTarget::Plant(def),
                                reserved: None,
                            },
                        );
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
        let gone: Vec<u32> = self
            .blueprints
            .values()
            .filter(|b| rect.contains(b.tile))
            .map(|b| b.id)
            .collect();
        for id in gone {
            self.remove_blueprint(world, content, id, true);
        }
    }

    fn remove_blueprint(&mut self, world: &mut World, content: &Content, id: u32, refund: bool) {
        let Some(bp) = self.blueprints.remove(&id) else {
            return;
        };
        if refund {
            if let Some(b) = &content.protos[bp.def].build {
                for (i, &(item, _)) in b.cost.iter().enumerate() {
                    if bp.delivered[i] > 0 {
                        self.items
                            .add(world, content, item, bp.delivered[i], bp.tile);
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
    pub fn place_blueprint(
        &mut self,
        world: &World,
        content: &Content,
        def: usize,
        tile: TilePos,
        seed: u64,
    ) -> Result<(), String> {
        let proto = &content.protos[def];
        let Some(build) = &proto.build else {
            return Err("not buildable".into());
        };
        if self
            .blueprints
            .values()
            .any(|b| b.tile == tile && content.protos[b.def].needs_support == proto.needs_support)
        {
            return Err("already planned".into());
        }
        // Roofs need a wall or rock nearby, built or planned.
        let range = proto.needs_support;
        if range > 0 {
            let r2 = (range * range) as i32;
            let planned = self.blueprints.values().any(|b| {
                let (dx, dy) = (b.tile.x - tile.x, b.tile.y - tile.y);
                content.protos[b.def].gives_support && dx * dx + dy * dy <= r2
            });
            if !planned && !objs::roof_supported(world, tile, range) {
                return Err(format!("A roof needs a wall or rock within {range} tiles"));
            }
        }
        let origin = match proto.anchor {
            Anchor::Tile => tile.origin(),
            Anchor::Center => {
                let (x, y) = tile.center();
                (x as i32, y as i32)
            }
        };
        if !objs::can_place(world, content, def, origin, seed) {
            return Err("blocked".into());
        }
        // Same layer already taken by something built here?
        let cells = proto.cells(seed);
        let clash = cells.iter().any(|c| {
            let (x, y) = (origin.0 + c.dx, origin.1 + c.dy);
            c.layer != Layer::Floor && world.get(x, y, c.layer) != NONE_MAT
        });
        if clash {
            return Err("something is already built there".into());
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
        Ok(())
    }

    // ------------------------------------------------------------ simulation

    /// `walk` scales walking speed (snow).
    pub fn tick(
        &mut self,
        world: &mut World,
        content: &Content,
        rng: &mut Rng,
        tick: u64,
        walk: f32,
    ) -> Vec<ColonyEvent> {
        let mut events = Vec::new();
        if tick % 30 == 0 {
            self.cleanup(world, content);
        }
        for i in 0..self.pawns.len() {
            let mut pawn = std::mem::take(&mut self.pawns[i]);
            let mut ctx = Ctx {
                world,
                content,
                rng,
                events: &mut events,
                walk,
            };
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
                mineable_cell(world, content, *t, &content.jobs.by_idx(d.job).targets).is_some()
                    || !world.is_loaded(t.origin().0, t.origin().1)
            }
            DesTarget::Plant(_) => true,
        });
        let stuck: Vec<(u32, usize, u32, TilePos)> = self
            .items
            .stacks
            .values()
            .filter(|s| {
                world.chunk(s.tile.chunk()).is_some()
                    && !world.walkable(s.tile)
                    && s.reserved.is_none()
            })
            .map(|s| (s.id, s.item, s.count, s.tile))
            .collect();
        for (id, item, count, tile) in stuck {
            self.items.take(id, count);
            self.items.add(world, content, item, count, tile);
        }
        self.stockpile
            .retain(|t| world.chunk(t.chunk()).is_none() || world.walkable(*t));
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
            // Put away anything still in hand before taking new work.
            if pawn.carrying.is_some() {
                pawn.job = self.find_store(pawn, ctx);
                if pawn.job.is_none() {
                    self.drop_carried(pawn, ctx);
                }
            }
            if pawn.job.is_none() {
                pawn.job = self.find_job(pawn, ctx);
            }
            if pawn.job.is_none() {
                // Nothing to do: look again a bit later (searching is the costly part).
                pawn.think = 30;
                if ctx.rng.chance(0.08) {
                    let start = pawn.tile();
                    let dx = ctx.rng.range_i32(-5, 5);
                    let dy = ctx.rng.range_i32(-5, 5);
                    let dest = start.offset(dx, dy);
                    if ctx.world.walkable(dest) {
                        pawn.job = Some(Job {
                            kind: JobKind::Wander { dest },
                            stage: 0,
                            timer: 0.0,
                            label: "Wandering".into(),
                        });
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
                JobKind::Supply { stack, item, drops } => {
                    self.unreserve_stack(stack, pawn.id);
                    // Drops not made yet no longer count as incoming.
                    let first = (job.stage as usize).saturating_sub(1);
                    for &(bp, n) in drops.iter().skip(first) {
                        self.uncount_incoming(ctx.content, bp, item, n);
                    }
                }
                JobKind::Store { .. } => {}
                JobKind::Construct { bp } => {
                    if let Some(b) = self.blueprints.get_mut(&bp) {
                        if b.reserved == Some(pawn.id) {
                            b.reserved = None;
                        }
                    }
                }
                JobKind::Wander { .. } | JobKind::Sleep { .. } | JobKind::Warm { .. } => {}
                JobKind::Eat { stack } => self.unreserve_stack(stack, pawn.id),
            }
        }
        pawn.path.clear();
        // Anything still carried is stored (or dropped) on the next think.
        pawn.think = 0;
    }

    fn cost_index(content: &Content, def: usize, item: usize) -> Option<usize> {
        content.protos[def]
            .build
            .as_ref()
            .and_then(|b| b.cost.iter().position(|c| c.0 == item))
    }

    fn uncount_incoming(&mut self, content: &Content, bp: u32, item: usize, n: u32) {
        if let Some(b) = self.blueprints.get_mut(&bp) {
            if let Some(i) = Self::cost_index(content, b.def, item) {
                b.incoming[i] = b.incoming[i].saturating_sub(n);
            }
        }
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
        let Some(&next) = pawn.path.last() else {
            return Ok(true);
        };
        let Some(info) = ctx.world.tile(next) else {
            return Err(());
        };
        if !info.walkable {
            return Err(());
        }
        let speed = ctx.content.creatures.by_idx(pawn.creature).speed * pawn.pace();
        let mut budget = speed * ctx.walk * TILE as f32 / TPS / info.cost.max(0.2);
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
        match find_path(
            w,
            pawn.tile(),
            |t| goal(t) && w.walkable(t),
            hint,
            MAX_PATH_NODES,
        ) {
            Some(p) => {
                pawn.path = p;
                true
            }
            None => false,
        }
    }

    // ------------------------------------------------------------ job search

    /// Search once for the nearest of many goals (stand tile -> value) and set the path.
    /// Falls back to an A* aimed at `hint` when nothing is close.
    fn search<T: Copy>(
        pawn: &mut Pawn,
        ctx: &Ctx,
        goals: &HashMap<TilePos, T>,
        ok: impl Fn(TilePos, T) -> bool,
        hint: TilePos,
    ) -> Option<T> {
        if goals.is_empty() {
            return None;
        }
        let w = &*ctx.world;
        let here = pawn.tile();
        let goal = |t: TilePos| goals.get(&t).copied().filter(|&v| ok(t, v));
        if let Some((v, p)) = find_nearest(w, here, goal, NEAR_NODES) {
            pawn.path = p;
            return Some(v);
        }
        let p = find_path(w, here, |t| goal(t).is_some(), hint, MAX_PATH_NODES)?;
        let v = goal(p.first().copied().unwrap_or(here))?;
        pawn.path = p;
        Some(v)
    }

    fn find_job(&mut self, pawn: &mut Pawn, ctx: &mut Ctx) -> Option<Job> {
        if pawn.drafted {
            return None;
        }
        let here = pawn.tile();
        // The pawn's own priorities first, then the job's global priority (stable sort).
        let mut order: Vec<(u8, usize)> = ctx
            .content
            .jobs_by_priority()
            .into_iter()
            .map(|ji| (pawn.work_priority(&ctx.content.jobs.by_idx(ji).id), ji))
            .filter(|(p, _)| *p > 0)
            .collect();
        order.sort_by_key(|x| x.0);
        for (_, ji) in order {
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

    fn find_designation(
        &mut self,
        pawn: &mut Pawn,
        ctx: &mut Ctx,
        job: usize,
        here: TilePos,
    ) -> Option<Job> {
        let jd = ctx.content.jobs.by_idx(job);
        let mut goals: HashMap<TilePos, TilePos> = HashMap::new();
        let mut hint: Option<(i32, TilePos)> = None;
        for (t, d) in &self.designations {
            if d.job != job || d.reserved.is_some() {
                continue;
            }
            let blocks = match &d.target {
                DesTarget::Object(o) => ctx
                    .world
                    .objects
                    .get(*o)
                    .is_some_and(|i| ctx.content.protos[i.def].blocks),
                DesTarget::Tile => true,
                DesTarget::Plant(_) => false,
            };
            for s in stand_tiles(*t, blocks) {
                goals.entry(s).or_insert(*t);
            }
            let dist = t.manhattan(here);
            if hint.is_none_or(|h| dist < h.0) {
                hint = Some((dist, *t));
            }
        }
        let t = Self::search(pawn, ctx, &goals, |_, _| true, hint?.1)?;
        self.designations.get_mut(&t).unwrap().reserved = Some(pawn.id);
        Some(Job {
            kind: JobKind::Work {
                at: t,
                driver: jd.driver,
            },
            stage: 0,
            timer: 0.0,
            label: jd.label.clone(),
        })
    }

    /// Builders either build a blueprint that has its materials, or bring
    /// materials to blueprints (one trip can supply several).
    fn find_construct(&mut self, pawn: &mut Pawn, ctx: &mut Ctx, here: TilePos) -> Option<Job> {
        let mut goals: HashMap<TilePos, BuildCand> = HashMap::new();
        let mut hint: Option<(i32, TilePos)> = None;
        let mut needed: BTreeSet<usize> = BTreeSet::new();
        // Unbuilt walls, so builders do not close themselves in.
        let walls: BTreeSet<TilePos> = self
            .blueprints
            .values()
            .filter(|b| ctx.content.protos[b.def].blocks)
            .map(|b| b.tile)
            .collect();
        for b in self.blueprints.values() {
            for (item, _) in b.needs(ctx.content) {
                needed.insert(item);
            }
            if b.reserved.is_some() || !b.ready(ctx.content) {
                continue;
            }
            // A roof waits until its wall is actually up.
            let range = ctx.content.protos[b.def].needs_support;
            if range > 0 && !objs::roof_supported(ctx.world, b.tile, range) {
                continue;
            }
            let blocks = ctx.content.protos[b.def].blocks;
            for s in stand_tiles(b.tile, blocks) {
                if !walls.contains(&s) {
                    goals.entry(s).or_insert(BuildCand::Build(b.id));
                }
            }
            let d = b.tile.manhattan(here);
            if hint.is_none_or(|h| d < h.0) {
                hint = Some((d, b.tile));
            }
        }
        for s in self.items.stacks.values() {
            if s.reserved.is_none() && needed.contains(&s.item) {
                goals.insert(s.tile, BuildCand::Stack(s.id));
                let d = s.tile.manhattan(here);
                if hint.is_none_or(|h| d < h.0) {
                    hint = Some((d, s.tile));
                }
            }
        }
        let hint = hint?.1;
        let world = &*ctx.world;
        let blueprints = &self.blueprints;
        let content = ctx.content;
        let open = std::cell::RefCell::new(HashMap::<TilePos, bool>::new());
        let ok = |t: TilePos, c: BuildCand| match c {
            BuildCand::Stack(_) => true,
            BuildCand::Build(id) => {
                // For walls: stand where you are not shut in.
                let blocks = blueprints
                    .get(&id)
                    .is_some_and(|b| content.protos[b.def].blocks);
                !blocks
                    || *open
                        .borrow_mut()
                        .entry(t)
                        .or_insert_with(|| open_area(world, &walls, t, 120))
            }
        };
        let found = Self::search(pawn, ctx, &goals, ok, hint)?;
        match found {
            BuildCand::Build(id) => {
                self.blueprints.get_mut(&id).unwrap().reserved = Some(pawn.id);
                let name = ctx
                    .content
                    .objects
                    .by_idx(self.blueprints[&id].def)
                    .name
                    .clone();
                Some(Job {
                    kind: JobKind::Construct { bp: id },
                    stage: 0,
                    timer: 0.0,
                    label: format!("Building {name}"),
                })
            }
            BuildCand::Stack(sid) => self.plan_supply(pawn, ctx, sid),
        }
    }

    /// Choose which blueprints one load from `sid` goes to, and reserve it all.
    fn plan_supply(&mut self, pawn: &mut Pawn, ctx: &mut Ctx, sid: u32) -> Option<Job> {
        let s = self.items.stacks.get(&sid)?.clone();
        let cap = s.count.min(ctx.content.item_rt[s.item].stack.max(1));
        let mut wants: Vec<(TilePos, u32, u32)> = self
            .blueprints
            .values()
            .filter_map(|b| {
                b.needs(ctx.content)
                    .into_iter()
                    .find(|n| n.0 == s.item)
                    .map(|n| (b.tile, b.id, n.1))
            })
            .collect();
        // Nearest blueprint to the stack first, then the ones closest to that one.
        let Some(first) = wants
            .iter()
            .min_by_key(|w| w.0.manhattan(s.tile))
            .map(|w| w.0)
        else {
            pawn.path.clear();
            return None;
        };
        wants.sort_by_key(|w| (w.0.manhattan(first), w.1));
        let mut drops = Vec::new();
        let mut total = 0;
        for (t, bp, need) in wants {
            if total >= cap || drops.len() >= MAX_DROPS || t.manhattan(first) > 16 {
                break;
            }
            let n = need.min(cap - total);
            total += n;
            drops.push((bp, n));
        }
        for &(bp, n) in &drops {
            let b = self.blueprints.get_mut(&bp).unwrap();
            let i = Self::cost_index(ctx.content, b.def, s.item).unwrap();
            b.incoming[i] += n;
        }
        self.items.stacks.get_mut(&sid).unwrap().reserved = Some(pawn.id);
        let name = ctx.content.items.by_idx(s.item).name.clone();
        Some(Job {
            kind: JobKind::Supply {
                stack: sid,
                item: s.item,
                drops,
            },
            stage: 0,
            timer: 0.0,
            label: format!("Hauling {name} to build"),
        })
    }

    /// Put carried items into the stockpile.
    fn find_store(&mut self, pawn: &mut Pawn, ctx: &mut Ctx) -> Option<Job> {
        let (item, _) = pawn.carrying?;
        let here = pawn.tile();
        if self.stockpile.contains(&here) {
            return None; // already in the stockpile: dropping here is fine
        }
        let dest = self.free_stockpile_tile(ctx, item, here)?;
        if !Self::path_to(pawn, ctx, move |t| t == dest, dest) {
            return None;
        }
        let name = ctx.content.items.by_idx(item).name.clone();
        Some(Job {
            kind: JobKind::Store { dest },
            stage: 0,
            timer: 0.0,
            label: format!("Storing {name}"),
        })
    }

    /// Loose items to stockpiles.
    fn find_haul(&mut self, pawn: &mut Pawn, ctx: &mut Ctx, here: TilePos) -> Option<Job> {
        if self.stockpile.is_empty() {
            return None;
        }
        let mut goals: HashMap<TilePos, u32> = HashMap::new();
        let mut hint: Option<(i32, TilePos)> = None;
        for s in self.items.stacks.values() {
            if s.reserved.is_none() && !self.stockpile.contains(&s.tile) {
                goals.insert(s.tile, s.id);
                let d = s.tile.manhattan(here);
                if hint.is_none_or(|h| d < h.0) {
                    hint = Some((d, s.tile));
                }
            }
        }
        let hint = hint?.1;
        // Only stacks that have somewhere to go.
        let has_room = |_: TilePos, sid: u32| {
            let s = &self.items.stacks[&sid];
            self.free_stockpile_tile(ctx, s.item, s.tile).is_some()
        };
        let sid = Self::search(pawn, ctx, &goals, has_room, hint)?;
        let (item, tile) = {
            let s = &self.items.stacks[&sid];
            (s.item, s.tile)
        };
        let dest = self.free_stockpile_tile(ctx, item, tile)?;
        self.items.stacks.get_mut(&sid).unwrap().reserved = Some(pawn.id);
        let name = ctx.content.items.by_idx(item).name.clone();
        Some(Job {
            kind: JobKind::HaulToStockpile { stack: sid, dest },
            stage: 0,
            timer: 0.0,
            label: format!("Hauling {name}"),
        })
    }

    fn free_stockpile_tile(&self, ctx: &Ctx, item: usize, from: TilePos) -> Option<TilePos> {
        let max = ctx.content.item_rt[item].stack;
        let taken: BTreeSet<TilePos> = self
            .pawns
            .iter()
            .filter_map(|p| match &p.job {
                Some(Job {
                    kind: JobKind::HaulToStockpile { dest, .. },
                    ..
                }) => Some(*dest),
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
                let d = if self.items.at(t).is_some() {
                    d - 1000
                } else {
                    d
                };
                if best.is_none_or(|(bd, _)| d < bd) {
                    best = Some((d, t));
                }
            }
        }
        best.map(|b| b.1)
    }

    // ------------------------------------------------------------ job execution

    fn run_job(&mut self, pawn: &mut Pawn, ctx: &mut Ctx) -> Step {
        let Some(job) = pawn.job.clone() else {
            return Step::Done;
        };
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
            JobKind::HaulToStockpile { stack, dest } => {
                self.run_haul_stockpile(pawn, ctx, *stack, *dest, job.stage)
            }
            JobKind::Supply { stack, item, drops } => {
                self.run_supply(pawn, ctx, *stack, *item, drops, job.stage)
            }
            JobKind::Store { dest } => {
                let dest = *dest;
                match Self::follow(pawn, ctx, move |t| t == dest, dest) {
                    Ok(true) => {
                        self.drop_carried(pawn, ctx);
                        Step::Done
                    }
                    Ok(false) => Step::Continue,
                    Err(()) => Step::Fail("no path"),
                }
            }
            JobKind::Construct { bp } => self.run_construct(pawn, ctx, *bp, job.stage),
            JobKind::Eat { stack } => self.run_eat(pawn, ctx, *stack, job.stage),
            JobKind::Sleep { bed, at } => Self::run_sleep(pawn, ctx, *bed, *at, job.stage),
            JobKind::Warm { at } => Self::run_warm(pawn, ctx, *at, job.stage),
        };
        step
    }

    fn run_eat(&mut self, pawn: &mut Pawn, ctx: &mut Ctx, stack: u32, stage: u8) -> Step {
        let Some(s) = self.items.get(stack) else {
            return Step::Fail("food gone");
        };
        let (tile, item) = (s.tile, s.item);
        if stage == 0 {
            return match Self::follow(pawn, ctx, move |t| t == tile, tile) {
                Ok(true) => {
                    self.set_stage(pawn, 1);
                    Step::Continue
                }
                Ok(false) => Step::Continue,
                Err(()) => Step::Fail("no path"),
            };
        }
        // Two seconds per item eaten.
        let t = Self::timer(pawn);
        *t += 1.0;
        if *t < TPS * 2.0 {
            return Step::Continue;
        }
        *t = 0.0;
        if self.items.take(stack, 1) == 0 {
            return Step::Done;
        }
        let nutrition = ctx.content.items.by_idx(item).nutrition;
        pawn.needs.food = (pawn.needs.food + nutrition).min(1.0);
        if pawn.needs.food >= 0.9 || self.items.get(stack).is_none() {
            Step::Done
        } else {
            Step::Continue
        }
    }

    fn run_sleep(
        pawn: &mut Pawn,
        ctx: &mut Ctx,
        bed: Option<ObjId>,
        at: TilePos,
        stage: u8,
    ) -> Step {
        if stage == 0 {
            return match Self::follow(pawn, ctx, move |t| t == at, at) {
                Ok(true) => {
                    if let Some(j) = &mut pawn.job {
                        j.stage = 1;
                    }
                    // Lie down on the bed itself.
                    if let Some(o) = bed.and_then(|b| ctx.world.objects.get(b)) {
                        let (min, max) = (o.min, o.max);
                        pawn.pos = (
                            (min.0 + max.0) as f32 / 2.0 + 0.5,
                            (min.1 + max.1) as f32 / 2.0 + 0.5,
                        );
                    }
                    Step::Continue
                }
                Ok(false) => Step::Continue,
                Err(()) => Step::Fail("no path"),
            };
        }
        if bed.is_some_and(|b| ctx.world.objects.get(b).is_none()) {
            return Step::Fail("bed gone");
        }
        // Rest itself is restored in `tick_needs`; wake when rested.
        if pawn.needs.rest >= 1.0 {
            Step::Done
        } else {
            Step::Continue
        }
    }

    fn run_warm(pawn: &mut Pawn, ctx: &mut Ctx, at: TilePos, stage: u8) -> Step {
        if stage == 0 {
            return match Self::follow(pawn, ctx, move |t| t == at, at) {
                Ok(true) => {
                    if let Some(j) = &mut pawn.job {
                        j.stage = 1;
                    }
                    Step::Continue
                }
                Ok(false) => Step::Continue,
                Err(()) => Step::Fail("no path"),
            };
        }
        if pawn.needs.warmth >= 0.9 {
            Step::Done
        } else {
            Step::Continue
        }
    }

    // ------------------------------------------------------------ needs

    /// Hunger, rest and warmth for creatures with `needs` data: they drop over
    /// time, and when one gets low the colonist drops its work to eat, sleep or
    /// warm up. Run before `tick`.
    pub fn tick_needs(
        &mut self,
        world: &mut World,
        content: &Content,
        rng: &mut Rng,
        env: &NeedsEnv,
    ) -> Vec<ColonyEvent> {
        let mut events = Vec::new();
        let mut dead = Vec::new();
        for i in 0..self.pawns.len() {
            let Some(def) = content
                .creatures
                .by_idx(self.pawns[i].creature)
                .needs
                .clone()
            else {
                continue;
            };
            if (env.tick + self.pawns[i].id as u64) % NEEDS_EVERY != 0 {
                continue;
            }
            let mut pawn = std::mem::take(&mut self.pawns[i]);
            let mut ctx = Ctx {
                world,
                content,
                rng,
                events: &mut events,
                walk: 1.0,
            };
            if let Some(cause) = self.update_needs(&mut pawn, &mut ctx, &def, env) {
                dead.push((pawn.id, cause));
            }
            self.pawns[i] = pawn;
        }
        for (id, cause) in dead {
            if let Some(p) = self.remove_pawn(world, content, rng, id) {
                events.push(ColonyEvent::Died {
                    name: p.name.clone(),
                    cause,
                    at: p.tile(),
                });
            }
        }
        events
    }

    fn update_needs(
        &mut self,
        pawn: &mut Pawn,
        ctx: &mut Ctx,
        def: &NeedsDef,
        env: &NeedsEnv,
    ) -> Option<&'static str> {
        let dt = NEEDS_EVERY as f32 / env.day_ticks.max(1) as f32; // in days
        let here = pawn.tile();
        let asleep = pawn.sleeping();
        let in_bed = asleep
            && matches!(
                &pawn.job,
                Some(Job {
                    kind: JobKind::Sleep { bed: Some(_), .. },
                    ..
                })
            );
        let burning = on_hot(ctx.world, ctx.content, pawn.pos);
        let n = &mut pawn.needs;
        n.felt = felt_temp(ctx.world, ctx.content, here, env);

        n.food = (n.food - dt / def.food_days.max(0.05) * if asleep { 0.6 } else { 1.0 }).max(0.0);
        n.rest = if asleep {
            (n.rest + dt / if in_bed { 0.3 } else { 0.45 }).min(1.0)
        } else {
            (n.rest - dt / def.awake_days.max(0.05)).max(0.0)
        };
        let low = def.comfort.0;
        if n.felt < low {
            n.warmth = (n.warmth - dt * (low - n.felt) * 0.15).max(0.0);
        } else {
            n.warmth = (n.warmth + dt * 8.0).min(1.0);
        }
        let tough = def.toughness.max(0.05);
        let mut hurt = 0.0;
        if n.food <= 0.0 {
            hurt += dt / tough;
        }
        if n.warmth <= 0.0 {
            hurt += dt * 2.0 / tough;
        }
        if burning {
            hurt += 0.05 / tough;
        }
        if hurt > 0.0 {
            n.health -= hurt;
        } else if n.food > 0.2 {
            n.health = (n.health + dt * 0.5).min(1.0);
        }
        if n.health <= 0.0 {
            return Some(if burning {
                "burned"
            } else if n.warmth <= 0.0 && n.food > 0.0 {
                "froze"
            } else {
                "starved"
            });
        }

        // ---- what to do about it
        if pawn.drafted {
            return None;
        }
        if burning {
            // Get off the fire first.
            if let Some(t) = nearest_tile(here, 6, |t| {
                ctx.world.walkable(t) && !on_hot(ctx.world, ctx.content, t.center())
            }) {
                self.end_job(pawn, ctx, false);
                pawn.job = Some(Job {
                    kind: JobKind::Wander { dest: t },
                    stage: 0,
                    timer: 0.0,
                    label: "Fleeing the fire".into(),
                });
            }
            return None;
        }
        let n = pawn.needs.clone();
        let night = env.dark > 0.62;
        let eating = matches!(
            &pawn.job,
            Some(Job {
                kind: JobKind::Eat { .. },
                ..
            })
        );
        let warming = matches!(
            &pawn.job,
            Some(Job {
                kind: JobKind::Warm { .. },
                ..
            })
        );
        // Hungry: eat (even wake up for it when starving).
        if n.food < 0.3 && !eating && (!asleep || n.food < 0.08) {
            if let Some(job) = self.find_food(pawn, ctx) {
                self.end_job(pawn, ctx, false);
                pawn.job = Some(job);
                return None;
            } else {
                self.warn(
                    ctx,
                    env,
                    WARN_FOOD,
                    format!(
                        "{} is hungry, but there is no food to reach. Harvest berries or hunt.",
                        pawn.name
                    ),
                );
            }
        }
        // Cold: find somewhere warmer (sleepers only wake when it gets bad).
        if n.warmth < 0.4 && !warming && !(asleep && n.warmth > 0.15) && env.tick >= n.warm_retry {
            if let Some(job) = self.find_warmth(pawn, ctx, env, def.comfort.0) {
                self.end_job(pawn, ctx, false);
                pawn.job = Some(job);
                return None;
            }
            // Searching is costly: wait an in-game hour before looking again.
            pawn.needs.warm_retry = env.tick + env.day_ticks / 24;
            self.warn(
                ctx,
                env,
                WARN_COLD,
                format!(
                    "{} is cold with nowhere warm to go. Build a roof and a campfire.",
                    pawn.name
                ),
            );
        }
        // Tired: sleep. At night anyone not well rested goes to bed.
        let busy_need = pawn.job.as_ref().is_some_and(|j| {
            matches!(
                j.kind,
                JobKind::Eat { .. } | JobKind::Sleep { .. } | JobKind::Warm { .. }
            )
        });
        let idle = pawn
            .job
            .as_ref()
            .is_none_or(|j| matches!(j.kind, JobKind::Wander { .. }));
        let wants_sleep =
            n.rest < 0.25 || (night && n.rest < 0.7) || (idle && night && n.rest < 0.95);
        if wants_sleep && !busy_need {
            let job = if n.rest <= 0.0 {
                // Collapses where it stands.
                Some(Job {
                    kind: JobKind::Sleep {
                        bed: None,
                        at: here,
                    },
                    stage: 0,
                    timer: 0.0,
                    label: "Collapsed from exhaustion".into(),
                })
            } else {
                self.find_bed(pawn, ctx, env)
            };
            if let Some(job) = job {
                self.end_job(pawn, ctx, false);
                pawn.job = Some(job);
            }
        }
        None
    }

    /// Say something at most twice a day per kind of warning.
    fn warn(&mut self, ctx: &mut Ctx, env: &NeedsEnv, kind: usize, msg: String) {
        let last = self.warned_at[kind];
        if last == 0 || env.tick >= last + env.day_ticks / 2 {
            self.warned_at[kind] = env.tick.max(1);
            ctx.events.push(ColonyEvent::Message(msg));
        }
    }

    fn find_food(&mut self, pawn: &mut Pawn, ctx: &mut Ctx) -> Option<Job> {
        let goals: HashMap<TilePos, u32> = self
            .items
            .stacks
            .values()
            .filter(|s| s.reserved.is_none() && ctx.content.items.by_idx(s.item).nutrition > 0.0)
            .map(|s| (s.tile, s.id))
            .collect();
        let hint = goals
            .keys()
            .min_by_key(|t| t.manhattan(pawn.tile()))
            .copied()?;
        let id = Self::search(pawn, ctx, &goals, |_, _| true, hint)?;
        let s = self.items.stacks.get_mut(&id)?;
        s.reserved = Some(pawn.id);
        let name = ctx.content.items.by_idx(s.item).name.clone();
        Some(Job {
            kind: JobKind::Eat { stack: id },
            stage: 0,
            timer: 0.0,
            label: format!("Eating {}", name.to_lowercase()),
        })
    }

    /// The nearest free bed, or else a sheltered spot on the ground.
    fn find_bed(&mut self, pawn: &mut Pawn, ctx: &mut Ctx, env: &NeedsEnv) -> Option<Job> {
        let here = pawn.tile();
        let taken: BTreeSet<ObjId> = self
            .pawns
            .iter()
            .filter_map(|p| match &p.job {
                Some(Job {
                    kind: JobKind::Sleep { bed: Some(b), .. },
                    ..
                }) => Some(*b),
                _ => None,
            })
            .collect();
        let area = TileRect::from_corners(here.offset(-40, -40), here.offset(40, 40));
        let mut goals: HashMap<TilePos, ObjId> = HashMap::new();
        for o in ctx.world.objects_in_rect(area) {
            let Some(inst) = ctx.world.objects.get(o) else {
                continue;
            };
            let proto = &ctx.content.protos[inst.def];
            if !inst.built || !proto.has_tag("bed") || taken.contains(&o) {
                continue;
            }
            let t = tile_of((inst.min.0 + inst.max.0) / 2, (inst.min.1 + inst.max.1) / 2);
            if env.keep_out.iter().any(|&(c, r)| c.chebyshev(t) <= r) {
                continue;
            }
            for s in stand_tiles(t, false) {
                if ctx.world.walkable(s) {
                    goals.entry(s).or_insert(o);
                }
            }
        }
        if let Some(hint) = goals.keys().min_by_key(|t| t.manhattan(here)).copied() {
            if let Some(bed) = Self::search(pawn, ctx, &goals, |_, _| true, hint) {
                let at = pawn.path.first().copied().unwrap_or(here);
                return Some(Job {
                    kind: JobKind::Sleep { bed: Some(bed), at },
                    stage: 0,
                    timer: 0.0,
                    label: "Sleeping in a bed".into(),
                });
            }
        }
        // No bed: under a roof if one is close, else right here.
        let roof = nearest_tile(here, 8, |t| {
            ctx.world.walkable(t) && roofed(ctx.world, ctx.content, t)
        });
        let at = match roof {
            Some(t) if t == here || Self::path_to(pawn, ctx, move |p| p == t, t) => t,
            _ => here,
        };
        Some(Job {
            kind: JobKind::Sleep { bed: None, at },
            stage: 0,
            timer: 0.0,
            label: "Sleeping on the ground".into(),
        })
    }

    /// The closest reachable spot that feels warmer than `low`.
    fn find_warmth(&self, pawn: &mut Pawn, ctx: &mut Ctx, env: &NeedsEnv, low: f32) -> Option<Job> {
        const R: i32 = 24;
        let here = pawn.tile();
        // Heat of every tile around, measured once.
        let reach = R + HEAT_RANGE;
        let mut sources = Vec::new();
        for y in -reach..=reach {
            for x in -reach..=reach {
                let h = tile_heat(ctx.world, ctx.content, here.offset(x, y));
                if h > 0.0 {
                    sources.push((x, y, h));
                }
            }
        }
        let mut spots: Vec<(i32, TilePos)> = Vec::new();
        for dy in -R..=R {
            for dx in -R..=R {
                let t = here.offset(dx, dy);
                if ctx.world.walkable(t) {
                    spots.push((dx.abs() + dy.abs(), t));
                }
            }
        }
        spots.sort_by_key(|s| s.0);
        let felt_at = |t: TilePos| {
            let (dx, dy) = (t.x - here.x, t.y - here.y);
            // Next to a fire, not in it.
            if sources.iter().any(|s| s.0 == dx && s.1 == dy) {
                return f32::MIN;
            }
            let warm: f32 = sources
                .iter()
                .filter(|s| (s.0 - dx).abs() <= HEAT_RANGE && (s.1 - dy).abs() <= HEAT_RANGE)
                .map(|s| heat_falloff(s.2, (s.0 - dx).abs().max((s.1 - dy).abs())))
                .sum();
            sheltered_temp(ctx.world, ctx.content, t, env) + warm.min(40.0)
        };
        // The closest comfortable spots first; if none is comfortable, the
        // warmest few that are clearly warmer than here.
        let now = pawn.needs.felt;
        let mut good: Vec<TilePos> = Vec::new();
        let mut warmer: Vec<(f32, TilePos)> = Vec::new();
        for (_, t) in spots {
            let f = felt_at(t);
            if f >= low + 1.0 {
                good.push(t);
                if good.len() >= 6 {
                    break;
                }
            } else if f >= now + 8.0 {
                warmer.push((f, t));
            }
        }
        if good.is_empty() {
            warmer.sort_by(|a, b| b.0.total_cmp(&a.0));
            good = warmer.into_iter().take(6).map(|w| w.1).collect();
        }
        let at = good
            .into_iter()
            .find(|&t| t == here || Self::path_to(pawn, ctx, move |p| p == t, t))?;
        Some(Job {
            kind: JobKind::Warm { at },
            stage: 0,
            timer: 0.0,
            label: "Warming up".into(),
        })
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
    fn follow(
        pawn: &mut Pawn,
        ctx: &mut Ctx,
        goal: impl Fn(TilePos) -> bool + Copy,
        hint: TilePos,
    ) -> Result<bool, ()> {
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
                if Self::path_to(pawn, ctx, goal, hint) {
                    Ok(false)
                } else {
                    Err(())
                }
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

    fn run_work(
        &mut self,
        pawn: &mut Pawn,
        ctx: &mut Ctx,
        at: TilePos,
        driver: JobDriver,
        stage: u8,
    ) -> Step {
        let Some(des) = self.designations.get(&at).cloned() else {
            return Step::Fail("order cancelled");
        };
        let jd = ctx.content.jobs.by_idx(des.job);
        let blocks = match &des.target {
            DesTarget::Object(o) => ctx
                .world
                .objects
                .get(*o)
                .is_some_and(|i| ctx.content.protos[i.def].blocks),
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
        let drop_tile = if ctx.world.walkable(at) {
            at
        } else {
            pawn.tile()
        };
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
                if cur.removed as f32 >= cur.total as f32 * proto.break_fraction
                    || objs::live_cells(ctx.world, o).is_empty()
                {
                    let c = objs::center(&cur);
                    objs::destroy(ctx.world, ctx.content, o);
                    if driver == JobDriver::Deconstruct {
                        if let Some(b) = &proto.build {
                            for &(item, n) in &b.cost {
                                let back = n.div_ceil(2);
                                if back > 0 {
                                    self.items
                                        .add(ctx.world, ctx.content, item, back, drop_tile);
                                }
                            }
                        }
                    } else {
                        for &(item, lo, hi) in &proto.drops {
                            let n = ctx.rng.range_i32(lo as i32, hi as i32) as u32;
                            if n > 0 {
                                self.items
                                    .add(ctx.world, ctx.content, item, n, cur.origin_tile());
                            }
                        }
                    }
                    ctx.events.push(ColonyEvent::Destroyed {
                        def: cur.def,
                        center: c,
                    });
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
                            let left: Vec<(usize, u32)> = pawn
                                .gathered
                                .iter()
                                .filter(|(_, n)| **n > 0)
                                .map(|(i, n)| (*i, *n))
                                .collect();
                            for (item, _) in left {
                                self.items.add(ctx.world, ctx.content, item, 1, drop_tile);
                                pawn.gathered.insert(item, 0);
                            }
                            ctx.events.push(ColonyEvent::Harvested {
                                def: inst.def,
                                center: objs::center(&inst),
                            });
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
                        ctx.world
                            .set(ctx.content, cx, cy, Layer::Solid, NONE_MAT, NO_OBJ);
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

    fn run_haul_stockpile(
        &mut self,
        pawn: &mut Pawn,
        ctx: &mut Ctx,
        stack: u32,
        dest: TilePos,
        stage: u8,
    ) -> Step {
        match stage {
            0 => {
                let Some(s) = self.items.get(stack) else {
                    return Step::Fail("items gone");
                };
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

    fn run_supply(
        &mut self,
        pawn: &mut Pawn,
        ctx: &mut Ctx,
        stack: u32,
        item: usize,
        drops: &[(u32, u32)],
        stage: u8,
    ) -> Step {
        if stage == 0 {
            let Some(s) = self.items.get(stack) else {
                return Step::Fail("items gone");
            };
            let tile = s.tile;
            return match Self::follow(pawn, ctx, move |t| t == tile, tile) {
                Ok(true) => {
                    let want: u32 = drops.iter().map(|d| d.1).sum();
                    let got = self.items.take(stack, want);
                    self.unreserve_stack(stack, pawn.id);
                    if got == 0 {
                        return Step::Fail("nothing to carry");
                    }
                    // Got less than planned: give up the last drops.
                    let mut left = got;
                    let mut keep = Vec::new();
                    for &(bp, n) in drops {
                        let give = n.min(left);
                        left -= give;
                        if give < n {
                            self.uncount_incoming(ctx.content, bp, item, n - give);
                        }
                        keep.push((bp, give));
                    }
                    if let Some(Job {
                        kind: JobKind::Supply { drops, .. },
                        ..
                    }) = &mut pawn.job
                    {
                        *drops = keep;
                    }
                    pawn.carrying = Some((item, got));
                    pawn.path.clear();
                    self.set_stage(pawn, 1);
                    Step::Continue
                }
                Ok(false) => Step::Continue,
                Err(()) => Step::Fail("no path"),
            };
        }
        let i = stage as usize - 1;
        let Some(&(bp, n)) = drops.get(i) else {
            return Step::Done;
        };
        let last = i + 1 >= drops.len();
        let next = |this: &mut Self, pawn: &mut Pawn| {
            pawn.path.clear();
            this.set_stage(pawn, stage + 1);
            if last { Step::Done } else { Step::Continue }
        };
        if n == 0 {
            return next(self, pawn);
        }
        let Some(b) = self.blueprints.get(&bp) else {
            return next(self, pawn);
        };
        let (bt, blocks) = (b.tile, ctx.content.protos[b.def].blocks);
        match Self::follow(pawn, ctx, near(bt, blocks), bt) {
            Ok(true) => {
                let have = pawn.carrying.map(|c| c.1).unwrap_or(0);
                let give = n.min(have);
                let b = self.blueprints.get_mut(&bp).unwrap();
                if let Some(ci) = Self::cost_index(ctx.content, b.def, item) {
                    b.delivered[ci] += give;
                    b.incoming[ci] = b.incoming[ci].saturating_sub(n);
                }
                pawn.carrying = if have > give {
                    Some((item, have - give))
                } else {
                    None
                };
                next(self, pawn)
            }
            Ok(false) => Step::Continue,
            Err(()) => {
                // Cannot reach this one: skip it.
                self.uncount_incoming(ctx.content, bp, item, n);
                next(self, pawn)
            }
        }
    }

    fn run_construct(&mut self, pawn: &mut Pawn, ctx: &mut Ctx, bp: u32, stage: u8) -> Step {
        let Some(b) = self.blueprints.get(&bp) else {
            return Step::Fail("blueprint gone");
        };
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
            b.obj = Some(objs::create_instance(
                ctx.world,
                ctx.content,
                def,
                b.origin,
                b.seed,
                true,
            ));
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
            let center = ctx
                .world
                .objects
                .get(id)
                .map(objs::center)
                .unwrap_or(b.origin);
            self.blueprints.remove(&bp);
            ctx.events.push(ColonyEvent::Built { def, center, id });
            return Step::Done;
        }
        Step::Continue
    }
}

fn mine_ok(world: &World, content: &Content, x: i32, y: i32, targets: &[String]) -> bool {
    let m = world.get(x, y, Layer::Solid);
    m != NONE_MAT
        && world.get_obj(x, y, Layer::Solid) == NO_OBJ
        && tags_match(targets, &content.mat_rt(m).tags)
}

/// A minable particle in this tile, if any.
pub fn mineable_cell(
    world: &World,
    content: &Content,
    t: TilePos,
    targets: &[String],
) -> Option<(i32, i32)> {
    World::tile_cells(t).find(|&(x, y)| mine_ok(world, content, x, y, targets))
}
