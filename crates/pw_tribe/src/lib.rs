//! Tribes: villages of people that live without the player.
//!
//! Each tribe runs its own colony (the same job system the player's
//! colonists use), so its people really chop trees, pick berries, haul and
//! build particle by particle. A **chief** decides once an in-game hour what
//! the village needs (food, wood, stone, houses, a hunt, a field) and gives
//! orders, weighted by the tribe's **culture**: how much it values each kind
//! of work. Culture is **learned**: every day the tribe compares how well it
//! is doing (food, people, homes, knowledge) with the day before and credits
//! the work it spent most effort on. Tribes **research** techs from data,
//! which unlock new abilities (hunting, farming, stonework) and building
//! plans. Fed and housed tribes have children; when a village grows large a
//! band leaves to found a **daughter tribe**, taking its knowledge along and
//! carrying a slightly mutated culture: tribes evolve.

use std::collections::BTreeSet;

use pw_colony::{Colony, ColonyEvent, Job, JobKind, Pawn, nearest_tile};
use pw_core::*;
use pw_life::{Death, Life};
use pw_objects as objs;
use pw_world::*;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------- culture

pub const FOCI: usize = 6;
pub const F_FOOD: usize = 0;
pub const F_WOOD: usize = 1;
pub const F_STONE: usize = 2;
pub const F_BUILD: usize = 3;
pub const F_HUNT: usize = 4;
pub const F_FARM: usize = 5;
pub const FOCUS_NAMES: [&str; FOCI] = ["Gathering food", "Cutting wood", "Breaking stone", "Building", "Hunting", "Farming"];
/// What a culture that values a focus is said to value.
pub const VALUE_NAMES: [&str; FOCI] = ["foraging", "woodcraft", "stonework", "building", "the hunt", "farming"];

pub const TRAITS: usize = 4;
pub const T_FERTILITY: usize = 0;
pub const T_WANDER: usize = 1;
pub const T_BOLD: usize = 2;
pub const T_CURIOUS: usize = 3;
pub const TRAIT_NAMES: [&str; TRAITS] = ["fertility", "wanderlust", "boldness", "curiosity"];

/// How fast culture shifts from a day's results.
const LEARN_RATE: f32 = 0.3;
/// Most tribes alive at once (each keeps its land loaded).
pub const MAX_TRIBES: usize = 8;
/// Food the chief aims to keep, in days.
const FOOD_GOAL: f32 = 5.0;
/// Days without enough food before someone starves.
const STARVE_DAYS: u32 = 3;
const NIGHT: f32 = 0.62;
const HUNTING: &str = "Hunting";
const HOMEWARD: &str = "Heading home";

const DEFAULT_SYLLABLES: [&str; 14] = ["ka", "ro", "mi", "ta", "lu", "ne", "so", "ar", "en", "ki", "va", "do", "ul", "she"];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Member {
    pub pawn: u32,
    pub age: u64,
    pub lifespan: u64,
    /// Days in a row without enough to eat.
    pub hunger: u32,
    /// Burns; heals over days.
    pub hurt: u32,
}

/// A building going up (or done) from a plan.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    pub plan: String,
    pub origin: TilePos,
    pub w: i32,
    pub h: i32,
    /// 0: walls, doors, furniture. 1: roofs.
    pub phase: u8,
    pub started: u64,
    pub done: bool,
}

impl Project {
    pub fn rect(&self) -> TileRect {
        TileRect::from_corners(self.origin, self.origin.offset(self.w - 1, self.h - 1))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Hunt {
    pub pawn: u32,
    pub prey: u32,
    pub until: u64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Tribe {
    pub id: u32,
    pub def_id: String,
    pub name: String,
    /// Tribe it split from.
    pub parent: Option<u32>,
    pub generation: u32,
    /// Banner colour (hue 0..1) and the hex colour drawn on its people.
    pub hue: f32,
    pub color: String,
    pub center: TilePos,
    pub radius: i32,
    pub colony: Colony,
    pub members: Vec<Member>,
    /// Learned value of each kind of work.
    pub culture: [f32; FOCI],
    /// Inherited, mutating traits.
    pub traits: [f32; TRAITS],
    pub techs: Vec<String>,
    pub research: Option<String>,
    pub points: f32,
    pub projects: Vec<Project>,
    pub hunts: Vec<Hunt>,
    /// Orders given per focus today (credit for learning).
    pub effort: [f32; FOCI],
    pub last_prosperity: f32,
    /// What the chief cares about most right now.
    pub focus: u8,
    pub founded: u64,
    pub born: u32,
    pub died: u32,
    pub hunted: u32,
    /// People at the end of each day.
    pub history: Vec<u32>,
    /// Notable moments, newest last.
    pub chronicle: Vec<String>,
    pub gone: Option<u64>,
    /// Beds in finished homes.
    pub beds: u32,
    pub think_at: u64,
    pub day_at: u64,
    #[serde(skip)]
    homes: Vec<TilePos>,
}

pub enum Founders {
    /// Brand new people.
    New(u32),
    /// A band that left another tribe.
    Band { parent: u32, pawns: Vec<Pawn>, members: Vec<Member>, food: Vec<(usize, u32)> },
}

#[derive(Debug)]
pub enum TribeEvent {
    Message(String),
    Founded { name: String },
    Discovered { tribe: String, tech: String },
    Lost { name: String },
    /// A band wants to leave and settle elsewhere; the game finds the land.
    Split { from: u32, pawns: Vec<Pawn>, members: Vec<Member>, food: Vec<(usize, u32)> },
    Colony(ColonyEvent),
}

/// The world as tribes see it this tick.
#[derive(Clone, Debug, Default)]
pub struct TribeEnv {
    pub tick: u64,
    pub day_ticks: u64,
    /// 0 day .. 1 night.
    pub dark: f32,
    pub walk: f32,
    /// Places tribes leave alone (the player's colony): centre and radius in tiles.
    pub keep_out: Vec<(TilePos, i32)>,
}

impl TribeEnv {
    fn hour(&self) -> u64 {
        (self.day_ticks / 24).max(1)
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Tribes {
    pub list: Vec<Tribe>,
    pub next_id: u32,
}

// ---------------------------------------------------------------- helpers

fn gauss(rng: &mut Rng) -> f32 {
    (rng.f32() + rng.f32() + rng.f32() - 1.5) / 1.5 * 1.7
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

pub fn make_name(def: &TribeDef, rng: &mut Rng, parts: (u32, u32)) -> String {
    let n = rng.range_i32(parts.0 as i32, parts.1 as i32).max(1);
    let mut s = String::new();
    for _ in 0..n {
        if def.syllables.is_empty() {
            s.push_str(DEFAULT_SYLLABLES[rng.below(DEFAULT_SYLLABLES.len())]);
        } else {
            s.push_str(&def.syllables[rng.below(def.syllables.len())]);
        }
    }
    capitalize(&s)
}

/// Hue (0..1) to a bright banner colour.
pub fn hue_color(h: f32) -> String {
    let h = h.rem_euclid(1.0) * 6.0;
    let (s, v) = (0.7f32, 0.9f32);
    let c = v * s;
    let x = c * (1.0 - ((h % 2.0) - 1.0).abs());
    let (r, g, b) = match h as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    let f = |u: f32| ((u + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    format!("#{:02x}{:02x}{:02x}", f(r), f(g), f(b))
}

fn tags_match(targets: &[String], tags: &[String]) -> bool {
    targets.is_empty() || targets.iter().any(|t| tags.contains(t))
}

/// Tiles of a plan: (tile, objects).
fn plan_cells(plan: &PlanDef, origin: TilePos) -> Vec<(TilePos, &Vec<String>)> {
    let h = plan.rows.len() as i32;
    let mut out = Vec::new();
    for (r, row) in plan.rows.iter().enumerate() {
        for (c, ch) in row.chars().enumerate() {
            if let Some(objs) = plan.palette.get(&ch) {
                out.push((origin.offset(c as i32, h - 1 - r as i32), objs));
            }
        }
    }
    out
}

fn plan_size(plan: &PlanDef) -> (i32, i32) {
    (plan.rows.iter().map(|r| r.chars().count()).max().unwrap_or(1) as i32, plan.rows.len().max(1) as i32)
}

fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

// ---------------------------------------------------------------- tribes

impl Tribes {
    pub fn alive(&self) -> usize {
        self.list.iter().filter(|t| t.gone.is_none()).count()
    }

    pub fn get(&self, id: u32) -> Option<&Tribe> {
        self.list.iter().find(|t| t.id == id)
    }

    pub fn living(&self) -> impl Iterator<Item = &Tribe> {
        self.list.iter().filter(|t| t.gone.is_none())
    }

    pub fn after_load(&mut self) {
        for t in &mut self.list {
            t.colony.after_load();
        }
    }

    /// Which tribe owns the land at a tile, if any.
    pub fn land_at(&self, t: TilePos) -> Option<&Tribe> {
        self.living().filter(|tr| tr.center.chebyshev(t) <= tr.radius).min_by_key(|tr| tr.center.chebyshev(t))
    }

    /// Start a tribe at `site`. Returns its id.
    #[allow(clippy::too_many_arguments)]
    pub fn found(
        &mut self,
        world: &mut World,
        content: &Content,
        rng: &mut Rng,
        def_id: &str,
        site: TilePos,
        tick: u64,
        day_ticks: u64,
        founders: Founders,
    ) -> Option<u32> {
        let def = content.tribes.get(def_id)?;
        let creature = content.creatures.idx(&def.creature)?;
        self.next_id += 1;
        let id = self.next_id;
        let parent = match &founders {
            Founders::Band { parent, .. } => self.get(*parent).cloned(),
            Founders::New(_) => None,
        };
        let (culture, traits, techs, hue, generation) = match &parent {
            Some(p) => {
                // Children of a culture: its values and traits, a little changed.
                let mut c = p.culture;
                for v in &mut c {
                    *v = (*v + gauss(rng) * 0.15).clamp(0.2, 3.0);
                }
                let mut t = p.traits;
                for v in &mut t {
                    *v = (*v + gauss(rng) * 0.1).clamp(0.3, 3.0);
                }
                (c, t, p.techs.clone(), p.hue + gauss(rng) * 0.08, p.generation + 1)
            }
            None => {
                let mut c = [1.0; FOCI];
                for v in &mut c {
                    *v = (1.0 + gauss(rng) * 0.12).max(0.3);
                }
                let mut t = [1.0; TRAITS];
                for v in &mut t {
                    *v = (1.0 + gauss(rng) * 0.1).max(0.3);
                }
                (c, t, def.start_techs.clone(), rng.f32(), 0)
            }
        };
        let name = make_name(def, rng, (2, 3));
        let color = hue_color(hue);
        let mut tribe = Tribe {
            id,
            def_id: def.id.clone(),
            name: name.clone(),
            parent: parent.as_ref().map(|p| p.id),
            generation,
            hue,
            color,
            center: site,
            radius: def.radius,
            colony: Colony::default(),
            members: Vec::new(),
            culture,
            traits,
            techs,
            research: None,
            points: 0.0,
            projects: Vec::new(),
            hunts: Vec::new(),
            effort: [0.0; FOCI],
            last_prosperity: 0.0,
            focus: F_FOOD as u8,
            founded: tick,
            born: 0,
            died: 0,
            hunted: 0,
            history: Vec::new(),
            chronicle: Vec::new(),
            gone: None,
            beds: 0,
            think_at: tick + 1,
            day_at: tick + day_ticks,
            homes: Vec::new(),
        };
        // The village centre: a totem, and a stockpile beside it.
        if let Some(totem) = def.totem.as_ref().and_then(|t| content.objects.idx(t)) {
            let seed = rng.next_u64();
            if objs::can_place(world, content, totem, site.origin(), seed) {
                objs::place(world, content, totem, site.origin(), seed, true);
            }
        }
        tribe.colony.set_stockpile(world, TileRect::from_corners(site.offset(2, -1), site.offset(4, 1)), true);
        let day = tick / day_ticks.max(1) + 1;
        match founders {
            Founders::New(n) => {
                let n = if n == 0 { rng.range_i32(def.members.0 as i32, def.members.1 as i32) as u32 } else { n };
                for i in 0..n {
                    let at = nearest_tile(site.offset(i as i32 % 3 - 1, -1 - i as i32 / 3), 5, |t| world.walkable(t)).unwrap_or(site);
                    let pawn = tribe.colony.spawn_pawn(content, creature, at, rng);
                    let span = rng.range_f32(def.lifespan_days.0, def.lifespan_days.1.max(def.lifespan_days.0)) * day_ticks as f32;
                    let age = (span * rng.range_f32(0.2, 0.45)) as u64;
                    tribe.dress(pawn, def, rng);
                    tribe.members.push(Member { pawn, age, lifespan: span as u64, hunger: 0, hurt: 0 });
                }
                for (item, n) in &def.start_items {
                    if let Some(i) = content.items.idx(item) {
                        tribe.colony.items.add(world, content, i, *n, site.offset(3, 0));
                    }
                }
                tribe.chronicle.push(format!("Day {day}: the {name} settled here."));
            }
            Founders::Band { pawns, members, food, .. } => {
                let from = parent.as_ref().map(|p| p.name.clone()).unwrap_or_default();
                for (pawn, mut m) in pawns.into_iter().zip(members) {
                    let pid = tribe.colony.adopt_pawn(pawn);
                    tribe.dress(pid, def, rng);
                    // They walk to their new home.
                    if let Some(p) = tribe.colony.pawns.iter_mut().find(|p| p.id == pid) {
                        p.job = Some(Job { kind: JobKind::Wander { dest: site }, stage: 0, timer: 0.0, label: HOMEWARD.into() });
                    }
                    m.pawn = pid;
                    tribe.members.push(m);
                }
                for (item, n) in food {
                    tribe.colony.items.add(world, content, item, n, site.offset(3, 0));
                }
                tribe.chronicle.push(format!("Day {day}: a band left the {from} and founded the {name} here."));
            }
        }
        if tribe.members.is_empty() {
            return None;
        }
        self.list.push(tribe);
        Some(id)
    }

    /// Put a band back into its tribe (no land was found for it).
    pub fn rejoin(&mut self, parent: u32, pawns: Vec<Pawn>, members: Vec<Member>, food: Vec<(usize, u32)>, world: &World, content: &Content) {
        let Some(t) = self.list.iter_mut().find(|t| t.id == parent) else { return };
        for (pawn, mut m) in pawns.into_iter().zip(members) {
            m.pawn = t.colony.adopt_pawn(pawn);
            t.members.push(m);
        }
        for (item, n) in food {
            t.colony.items.add(world, content, item, n, t.center.offset(3, 0));
        }
    }

    pub fn tick(&mut self, world: &mut World, content: &Content, rng: &mut Rng, env: &TribeEnv, life: &mut Life) -> Vec<TribeEvent> {
        let mut events = Vec::new();
        let mut room = self.alive() < MAX_TRIBES;
        let centers: Vec<(u32, TilePos)> = self.living().map(|t| (t.id, t.center)).collect();
        for t in &mut self.list {
            if t.gone.is_some() {
                continue;
            }
            let others: Vec<TilePos> = centers.iter().filter(|c| c.0 != t.id).map(|c| c.1).collect();
            if t.tick(world, content, rng, env, life, &others, room, &mut events) {
                room = false; // one band at a time
            }
        }
        events
    }
}

impl Tribe {
    pub fn def<'a>(&self, content: &'a Content) -> Option<&'a TribeDef> {
        content.tribes.get(&self.def_id)
    }

    pub fn member(&self, pawn: u32) -> Option<&Member> {
        self.members.iter().find(|m| m.pawn == pawn)
    }

    /// Give a pawn a person's name and the tribe's colours.
    fn dress(&mut self, pawn: u32, def: &TribeDef, rng: &mut Rng) {
        let name = make_name(def, rng, (2, 2));
        let color = self.color.clone();
        if let Some(p) = self.colony.pawns.iter_mut().find(|p| p.id == pawn) {
            p.name = name;
            p.slots.insert("band".into(), color);
        }
    }

    fn note(&mut self, day: u64, s: impl Into<String>) {
        self.chronicle.push(format!("Day {day}: {}", s.into()));
        if self.chronicle.len() > 40 {
            self.chronicle.remove(0);
        }
    }

    /// What its people can do: union of what its techs unlock.
    pub fn can(&self, def: &TribeDef, ability: &str) -> bool {
        def.techs.iter().any(|t| self.techs.contains(&t.id) && t.unlocks.iter().any(|u| u == ability))
    }

    fn food_items(def: &TribeDef, content: &Content) -> Vec<(usize, f32)> {
        def.food.iter().filter_map(|(id, v)| content.items.idx(id).map(|i| (i, *v))).collect()
    }

    /// Meals in store.
    pub fn food(&self, def: &TribeDef, content: &Content) -> f32 {
        Self::food_items(def, content).iter().map(|&(i, v)| self.colony.items.total(i) as f32 * v).sum()
    }

    /// Days the store would last.
    pub fn food_days(&self, def: &TribeDef, content: &Content) -> f32 {
        let need = self.members.len() as f32 * def.meals_per_day.max(0.01);
        if need <= 0.0 { 99.0 } else { self.food(def, content) / need }
    }

    pub fn item_total(&self, content: &Content, id: &str) -> u32 {
        content.items.idx(id).map(|i| self.colony.items.total(i)).unwrap_or(0)
    }

    /// How many people the village has room for.
    pub fn capacity(&self) -> u32 {
        2 + self.beds
    }

    /// One tick. Returns true if a band left.
    #[allow(clippy::too_many_arguments)]
    fn tick(
        &mut self,
        world: &mut World,
        content: &Content,
        rng: &mut Rng,
        env: &TribeEnv,
        life: &mut Life,
        others: &[TilePos],
        room: bool,
        events: &mut Vec<TribeEvent>,
    ) -> bool {
        let Some(def) = content.tribes.get(&self.def_id) else { return false };
        let day = env.tick / env.day_ticks.max(1) + 1;
        let night = env.dark > NIGHT;
        self.send_home(world, rng, night);
        for e in self.colony.tick(world, content, rng, env.tick, env.walk) {
            if !matches!(e, ColonyEvent::Message(_)) {
                events.push(TribeEvent::Colony(e));
            }
        }
        if env.tick.is_multiple_of(5) {
            self.hunt(world, content, rng, env, life, day);
        }
        let mut deaths: Vec<(u32, &'static str)> = Vec::new();
        for m in &mut self.members {
            m.age += 1;
            if m.age > m.lifespan {
                deaths.push((m.pawn, "died of old age"));
            }
        }
        if env.tick.is_multiple_of(10) {
            for p in &self.colony.pawns {
                let (x, y) = (p.pos.0 as i32, p.pos.1 as i32);
                let hot = [Layer::Floor, Layer::Solid, Layer::Overlay].iter().any(|&l| content.mat_rt(world.get(x, y, l)).has_tag("hot"));
                if hot
                    && let Some(m) = self.members.iter_mut().find(|m| m.pawn == p.id) {
                        m.hurt += 1;
                        if m.hurt >= 5 {
                            deaths.push((p.id, "burned to death"));
                        }
                    }
            }
        }
        if env.tick >= self.think_at {
            self.think_at = env.tick + env.hour();
            self.think(def, world, content, rng, env, life, others);
        }
        let mut split = false;
        if env.tick >= self.day_at {
            self.day_at = env.tick + env.day_ticks;
            let (starved, band) = self.daily(def, world, content, rng, env, events, room);
            for p in starved {
                deaths.push((p, "starved"));
            }
            if let Some(e) = band {
                events.push(e);
                split = true;
            }
        }
        deaths.sort();
        deaths.dedup_by_key(|d| d.0);
        for (pawn, how) in deaths {
            self.lose(world, content, rng, pawn, how, day);
        }
        if self.members.is_empty() && self.gone.is_none() {
            self.gone = Some(env.tick);
            self.note(day, "the last of them is gone.");
            let r = TileRect::from_corners(self.center.offset(-self.radius, -self.radius), self.center.offset(self.radius, self.radius));
            self.colony.cancel(world, content, r);
            self.colony.designations.clear();
            self.hunts.clear();
            events.push(TribeEvent::Lost { name: self.name.clone() });
        }
        split
    }

    fn lose(&mut self, world: &mut World, content: &Content, rng: &mut Rng, pawn: u32, how: &str, day: u64) {
        if let Some(p) = self.colony.remove_pawn(world, content, rng, pawn) {
            self.note(day, format!("{} {how}.", p.name));
        }
        self.members.retain(|m| m.pawn != pawn);
        self.hunts.retain(|h| h.pawn != pawn);
        self.died += 1;
    }

    /// Nights are spent at home; people who wandered off come back.
    fn send_home(&mut self, world: &World, rng: &mut Rng, night: bool) {
        let center = self.center;
        let far = self.radius + 6;
        for (i, p) in self.colony.pawns.iter_mut().enumerate() {
            if p.job.is_some() {
                continue;
            }
            let here = p.tile();
            if night {
                let home = if self.homes.is_empty() { center.offset(i as i32 % 3 - 1, 1 + i as i32 / 3 % 2) } else { self.homes[i % self.homes.len()] };
                if here.chebyshev(home) <= 1 || p.think > 0 {
                    p.think = p.think.max(20);
                } else if world.walkable(home) {
                    p.job = Some(Job { kind: JobKind::Wander { dest: home }, stage: 0, timer: 0.0, label: HOMEWARD.into() });
                    p.think = 0;
                } else {
                    p.think = 40 + rng.below(20) as u32;
                }
            } else if here.chebyshev(center) > far && p.think == 0 {
                p.job = Some(Job { kind: JobKind::Wander { dest: center.offset(0, -2) }, stage: 0, timer: 0.0, label: HOMEWARD.into() });
            }
        }
    }

    /// Hunters chase their prey and try to bring it down.
    fn hunt(&mut self, _world: &mut World, content: &Content, rng: &mut Rng, env: &TribeEnv, life: &mut Life, day: u64) {
        let mut hunts = std::mem::take(&mut self.hunts);
        let bold = self.traits[T_BOLD];
        let mut caught: Vec<(String, TilePos, usize)> = Vec::new();
        hunts.retain(|h| {
            let Some(pawn) = self.colony.pawns.iter_mut().find(|p| p.id == h.pawn) else { return false };
            let hunting = pawn.job.as_ref().is_some_and(|j| j.label.starts_with(HUNTING));
            if pawn.job.is_some() && !hunting {
                return false; // busy with something else now
            }
            let prey = life.get(h.prey).filter(|c| c.dead.is_none()).map(|c| (c.pos, c.tile(), c.def));
            let Some((ppos, ptile, pdef)) = prey.filter(|_| env.tick <= h.until) else {
                if hunting {
                    pawn.job = None;
                    pawn.path.clear();
                }
                return false;
            };
            if dist(pawn.pos, ppos) <= TILE as f32 * 1.7 {
                if rng.chance((0.22 * bold).clamp(0.05, 0.6)) {
                    life.kill(h.prey, Death::Hunted);
                    caught.push((pawn.name.clone(), pawn.tile(), pdef));
                    pawn.job = None;
                    pawn.path.clear();
                    return false;
                }
                return true;
            }
            let retarget = match &pawn.job {
                Some(Job { kind: JobKind::Wander { dest }, .. }) => dest.chebyshev(ptile) > 1,
                _ => true,
            };
            if retarget {
                let name = content.creatures.by_idx(pdef).name.to_lowercase();
                pawn.job = Some(Job { kind: JobKind::Wander { dest: ptile }, stage: 0, timer: 0.0, label: format!("{HUNTING} {name}") });
                pawn.path.clear();
            }
            true
        });
        self.hunts = hunts;
        for (who, at, def) in caught {
            let d = content.creatures.by_idx(def);
            if let Some(l) = &d.life {
                for (item, n) in &l.remains {
                    if let Some(i) = content.items.idx(item) {
                        self.colony.items.add(_world, content, i, *n, at);
                    }
                }
            }
            self.hunted += 1;
            self.effort[F_HUNT] += 1.0;
            if self.hunted == 1 {
                self.note(day, format!("{who} brought down the tribe's first {}.", d.name.to_lowercase()));
            }
        }
    }

    // ------------------------------------------------------------ the chief

    #[allow(clippy::too_many_arguments)]
    fn think(&mut self, def: &TribeDef, world: &mut World, content: &Content, rng: &mut Rng, env: &TribeEnv, life: &Life, others: &[TilePos]) {
        let day = env.tick / env.day_ticks.max(1) + 1;
        self.refresh_homes(def, world, content);
        self.advance_projects(def, world, content, rng, env, day);
        if env.dark > NIGHT {
            return;
        }
        let food_days = self.food_days(def, content);
        let food_need = ((FOOD_GOAL - food_days) / FOOD_GOAL).clamp(0.0, 1.0);
        let (mut bp_wood, mut bp_stone) = (0u32, 0u32);
        let wood_i = content.items.idx(&def.wood);
        let stone_i = content.items.idx(&def.stone);
        for b in self.colony.blueprints.values() {
            for (item, n) in b.needs(content) {
                if Some(item) == wood_i {
                    bp_wood += n;
                }
                if Some(item) == stone_i {
                    bp_stone += n;
                }
            }
        }
        let wood = wood_i.map(|i| self.colony.items.total(i)).unwrap_or(0) as f32;
        let stone = stone_i.map(|i| self.colony.items.total(i)).unwrap_or(0) as f32;
        let building = self.projects.iter().any(|p| !p.done);
        let has_fire = self.projects.iter().any(|p| def.plans.iter().any(|pl| pl.id == p.plan && pl.role == "fire"));
        let people = self.members.len() as u32;

        let mut need = [0.0f32; FOCI];
        need[F_FOOD] = if self.can(def, "harvest") && food_days < FOOD_GOAL * 2.0 { food_need.max(0.12) } else { 0.0 };
        need[F_WOOD] = if self.can(def, "chop") { (1.0 - wood / (25.0 + bp_wood as f32)).clamp(0.05, 1.0) } else { 0.0 };
        need[F_STONE] = if self.can(def, "quarry") { (1.0 - stone / (15.0 + bp_stone as f32)).clamp(0.0, 1.0) * 0.8 } else { 0.0 };
        need[F_BUILD] = if building {
            0.0
        } else if !has_fire {
            1.0
        } else if people >= self.capacity() {
            0.9
        } else if people + 1 >= self.capacity() {
            0.35
        } else {
            0.03
        };
        // Hunt when food runs short, not for sport.
        need[F_HUNT] = if self.can(def, "hunt") && self.hunts.is_empty() { food_need } else { 0.0 };
        need[F_FARM] = if self.can(def, "farm") && def.crop.is_some() { food_need * 0.7 + 0.1 } else { 0.0 };

        let mut order: Vec<(usize, f32)> = (0..FOCI).map(|f| (f, need[f] * self.culture[f])).collect();
        order.sort_by(|a, b| b.1.total_cmp(&a.1));
        self.focus = order[0].0 as u8;
        let cap = (people as usize * 2 + 2).max(4);
        let mut acted = 0;
        for &(f, score) in &order {
            if score < 0.12 || acted >= 3 {
                break;
            }
            let open = self.colony.designations.len();
            let room = cap.saturating_sub(open);
            let n = match f {
                F_FOOD if room > 0 => self.order_objects(world, content, &def.jobs.harvest, true, 2.min(room), env),
                F_WOOD if room > 0 => self.order_objects(world, content, &def.jobs.chop, false, 2.min(room), env),
                F_STONE if room > 0 => self.order_objects(world, content, &def.jobs.quarry, false, 1, env),
                F_FARM if room > 0 => self.order_planting(world, content, def, 2.min(room), env),
                F_BUILD => self.start_project(def, world, content, rng, env, others) as usize * 3,
                F_HUNT => self.start_hunt(def, content, life, env) as usize,
                _ => 0,
            };
            if n > 0 {
                self.effort[f] += n as f32;
                acted += 1;
            }
        }
    }

    fn in_reach(&self, t: TilePos, env: &TribeEnv) -> bool {
        let d = t.chebyshev(self.center);
        d <= self.radius && !env.keep_out.iter().any(|&(c, r)| c.chebyshev(t) <= r)
    }

    /// Order work on the nearest few objects a job targets. Returns how many.
    fn order_objects(&mut self, world: &World, content: &Content, job: &str, need_yield: bool, k: usize, env: &TribeEnv) -> usize {
        let Some(j) = content.jobs.idx(job) else { return 0 };
        let targets = &content.jobs.by_idx(j).targets;
        let r = self.radius;
        let rect = TileRect::from_corners(self.center.offset(-r, -r), self.center.offset(r, r));
        let mut cands: Vec<(i32, TilePos)> = Vec::new();
        for o in world.objects_in_rect(rect) {
            let Some(inst) = world.objects.get(o) else { continue };
            let p = &content.protos[inst.def];
            let t = inst.origin_tile();
            if inst.built || p.has_tag("plant") || !tags_match(targets, &p.tags) || self.colony.designations.contains_key(&t) || !self.in_reach(t, env) {
                continue;
            }
            if need_yield && !objs::has_yield(world, content, o) {
                continue;
            }
            // Keep the trees right at the totem; prefer what is near.
            let d = t.chebyshev(self.center);
            if d < 2 {
                continue;
            }
            cands.push((d, t));
        }
        cands.sort();
        let mut n = 0;
        for &(_, t) in cands.iter().take(k) {
            n += self.colony.designate(world, content, j, TileRect::from_corners(t, t), None);
        }
        n
    }

    fn order_planting(&mut self, world: &World, content: &Content, def: &TribeDef, k: usize, env: &TribeEnv) -> usize {
        let (Some(j), Some(crop)) = (content.jobs.idx(&def.jobs.plant), def.crop.as_ref().and_then(|c| content.objects.idx(c))) else { return 0 };
        let mut n = 0;
        // A field ring just outside the houses.
        for r in 5..self.radius.min(10) {
            for i in 0..(r * 8) {
                if n >= k {
                    return n;
                }
                let a = i as f32 / (r * 8) as f32 * std::f32::consts::TAU;
                let t = self.center.offset((a.cos() * r as f32).round() as i32, (a.sin() * r as f32).round() as i32);
                if !self.in_reach(t, env) || self.colony.designations.contains_key(&t) || !world.objects_in_tile(t).is_empty() {
                    continue;
                }
                if self.projects.iter().any(|p| { let rr = p.rect(); rr.contains(t) }) || self.colony.stockpile.contains(&t) {
                    continue;
                }
                n += self.colony.designate(world, content, j, TileRect::from_corners(t, t), Some(crop));
            }
        }
        n
    }

    fn start_hunt(&mut self, def: &TribeDef, content: &Content, life: &Life, env: &TribeEnv) -> bool {
        if !self.hunts.is_empty() {
            return false;
        }
        let Some(pawn) = self.colony.pawns.iter().find(|p| p.job.as_ref().is_none_or(|j| j.label == "Wandering")) else { return false };
        let reach = (self.radius + 8) as f32 * TILE as f32;
        let (cx, cy) = self.center.center();
        let prey = life
            .creatures
            .iter()
            .filter(|c| c.dead.is_none() && dist(c.pos, (cx, cy)) <= reach)
            .filter(|c| content.creatures.by_idx(c.def).life.as_ref().is_some_and(|l| l.tags.iter().any(|t| def.prey.contains(t))))
            .min_by(|a, b| dist(a.pos, pawn.pos).total_cmp(&dist(b.pos, pawn.pos)));
        let Some(prey) = prey else { return false };
        self.hunts.push(Hunt { pawn: pawn.id, prey: prey.id, until: env.tick + env.hour() * 4 });
        true
    }

    // ------------------------------------------------------------ building

    /// Beds in finished homes, and where people sleep.
    fn refresh_homes(&mut self, def: &TribeDef, world: &World, content: &Content) {
        let mut beds = 0;
        let mut homes = Vec::new();
        for p in self.projects.iter().filter(|p| p.done) {
            let Some(plan) = def.plans.iter().find(|pl| pl.id == p.plan) else { continue };
            if plan.role != "home" {
                continue;
            }
            for o in world.objects_in_rect(p.rect()) {
                if let Some(inst) = world.objects.get(o)
                    && inst.built && content.protos[inst.def].has_tag("bed") {
                        beds += 1;
                    }
            }
            for (t, objs) in plan_cells(plan, p.origin) {
                let roof_only = objs.iter().all(|o| content.objects.idx(o).is_some_and(|d| content.protos[d].needs_support > 0));
                if roof_only && world.walkable(t) {
                    homes.push(t);
                }
            }
        }
        self.beds = beds;
        self.homes = homes;
    }

    fn place_phase(&mut self, plan: &PlanDef, origin: TilePos, phase: u8, world: &World, content: &Content, rng: &mut Rng) -> usize {
        let mut n = 0;
        for (t, list) in plan_cells(plan, origin) {
            for id in list {
                let Some(d) = content.objects.idx(id) else { continue };
                if (content.protos[d].needs_support > 0) != (phase == 1) {
                    continue;
                }
                if self.colony.place_blueprint(world, content, d, t, rng.next_u64()).is_ok() {
                    n += 1;
                }
            }
        }
        n
    }

    fn advance_projects(&mut self, def: &TribeDef, world: &mut World, content: &Content, rng: &mut Rng, env: &TribeEnv, day: u64) {
        for i in 0..self.projects.len() {
            if self.projects[i].done {
                continue;
            }
            let p = self.projects[i].clone();
            let rect = p.rect();
            let Some(plan) = def.plans.iter().find(|pl| pl.id == p.plan) else {
                self.projects[i].done = true;
                continue;
            };
            let pending = self.colony.blueprints.values().any(|b| rect.contains(b.tile));
            if pending {
                if env.tick.saturating_sub(p.started) > env.day_ticks * 6 {
                    // Stuck for days: give up on it.
                    self.colony.cancel(world, content, rect);
                    self.projects[i].done = true;
                    self.note(day, format!("gave up building a {}.", plan.name.to_lowercase()));
                }
                continue;
            }
            if p.phase == 0 {
                self.projects[i].phase = 1;
                if self.place_phase(plan, p.origin, 1, world, content, rng) > 0 {
                    continue;
                }
            }
            self.projects[i].done = true;
            let name = plan.name.to_lowercase();
            self.note(day, format!("finished a {name}."));
        }
    }

    /// Choose what to build next and where. Returns true if work began.
    fn start_project(&mut self, def: &TribeDef, world: &World, content: &Content, rng: &mut Rng, env: &TribeEnv, others: &[TilePos]) -> bool {
        let known: Vec<&PlanDef> = def.plans.iter().filter(|p| p.tech.as_ref().is_none_or(|t| self.techs.contains(t))).collect();
        let has_fire = self.projects.iter().any(|p| known.iter().any(|pl| pl.id == p.plan && pl.role == "fire"));
        let plan = match known.iter().find(|p| p.role == "fire").filter(|_| !has_fire) {
            Some(p) => *p,
            None => {
                let homes: Vec<&&PlanDef> = known.iter().filter(|p| p.role == "home").collect();
                if homes.is_empty() {
                    return false;
                }
                // The most advanced home they know, now and then an older one.
                let k = if homes.len() > 1 && rng.chance(0.25) { homes.len() - 2 } else { homes.len() - 1 };
                *homes[k]
            }
        };
        let (w, h) = plan_size(plan);
        let Some(origin) = self.find_site(world, w, h, env, others) else { return false };
        let n = self.place_phase(plan, origin, 0, world, content, rng);
        if n == 0 {
            return false;
        }
        self.projects.push(Project { plan: plan.id.clone(), origin, w, h, phase: 0, started: env.tick, done: false });
        true
    }

    fn find_site(&self, world: &World, w: i32, h: i32, env: &TribeEnv, others: &[TilePos]) -> Option<TilePos> {
        let taken: Vec<TileRect> = self.projects.iter().map(|p| TileRect::from_corners(p.origin.offset(-1, -1), p.origin.offset(p.w, p.h))).collect();
        for r in 2..(self.radius - 3).max(3) {
            for k in 0..16 {
                let a = k as f32 / 16.0 * std::f32::consts::TAU + r as f32 * 0.37;
                let c = self.center.offset((a.cos() * r as f32).round() as i32, (a.sin() * r as f32).round() as i32);
                let origin = c.offset(-w / 2, -h / 2);
                let outer = TileRect::from_corners(origin.offset(-1, -1), origin.offset(w, h));
                let inner = TileRect::from_corners(origin, origin.offset(w - 1, h - 1));
                if !self.in_reach(outer.min, env) || !self.in_reach(outer.max, env) {
                    continue;
                }
                if outer.iter().any(|t| (t.chebyshev(self.center) <= 1) || !world.walkable(t) || self.colony.stockpile.contains(&t)) {
                    continue;
                }
                if taken.iter().any(|tr| tr.iter().any(|t| outer.contains(t))) || others.iter().any(|o| outer.contains(*o)) {
                    continue;
                }
                if inner.iter().any(|t| self.colony.items.at(t).is_some()) || !world.objects_in_rect(inner).is_empty() {
                    continue;
                }
                return Some(origin);
            }
        }
        None
    }

    // ------------------------------------------------------------ days

    /// Eat, age, have children, learn, research, maybe split.
    /// Returns pawns that starved, and a band that wants to leave.
    #[allow(clippy::too_many_arguments)]
    fn daily(
        &mut self,
        def: &TribeDef,
        world: &mut World,
        content: &Content,
        rng: &mut Rng,
        env: &TribeEnv,
        events: &mut Vec<TribeEvent>,
        room: bool,
    ) -> (Vec<u32>, Option<TribeEvent>) {
        let day = env.tick / env.day_ticks.max(1) + 1;
        // ---- eat: the hungriest first
        let per = def.meals_per_day.max(0.01);
        let mut meals = 0.0f32;
        let want = self.members.len() as f32 * per;
        for (item, value) in Self::food_items(def, content) {
            let stacks: Vec<u32> = self.colony.items.stacks.values().filter(|s| s.item == item && s.reserved.is_none()).map(|s| s.id).collect();
            for s in stacks {
                while meals < want - 0.001 {
                    if self.colony.items.take(s, 1) == 0 {
                        break;
                    }
                    meals += value;
                }
            }
        }
        let mut fed = (meals / per + 0.001).floor() as usize;
        self.members.sort_by(|a, b| b.hunger.cmp(&a.hunger));
        let mut starved = Vec::new();
        let mut hungry = false;
        for m in &mut self.members {
            m.hurt = m.hurt.saturating_sub(2);
            if fed > 0 {
                fed -= 1;
                m.hunger = 0;
            } else {
                m.hunger += 1;
                hungry = true;
                if m.hunger >= STARVE_DAYS {
                    starved.push(m.pawn);
                }
            }
        }
        if hungry {
            // Hard times teach a people to value food.
            self.culture[F_FOOD] = (self.culture[F_FOOD] + 0.08).min(3.0);
            self.culture[F_HUNT] = (self.culture[F_HUNT] + 0.05).min(3.0);
            self.culture[F_FARM] = (self.culture[F_FARM] + 0.05).min(3.0);
        }

        // ---- children
        let people = self.members.len() as u32;
        let food_days = self.food_days(def, content);
        if people >= 2 && people < def.max_members && people < self.capacity() && food_days >= 2.0 && rng.chance((0.45 * self.traits[T_FERTILITY]).min(0.9))
            && let Some(creature) = content.creatures.idx(&def.creature) {
                let at = nearest_tile(self.homes.first().copied().unwrap_or(self.center.offset(0, 1)), 4, |t| world.walkable(t)).unwrap_or(self.center);
                let pawn = self.colony.spawn_pawn(content, creature, at, rng);
                self.dress(pawn, def, rng);
                let span = rng.range_f32(def.lifespan_days.0, def.lifespan_days.1.max(def.lifespan_days.0)) * env.day_ticks as f32;
                self.members.push(Member { pawn, age: 0, lifespan: span as u64, hunger: 0, hurt: 0 });
                self.born += 1;
                let name = self.colony.pawns.last().map(|p| p.name.clone()).unwrap_or_default();
                self.note(day, format!("{name} was born."));
            }

        // ---- research
        self.points += self.members.len() as f32 * 1.5 * self.traits[T_CURIOUS];
        if self.research.as_ref().is_none_or(|r| self.techs.contains(r)) {
            let open: Vec<&TechDef> = def
                .techs
                .iter()
                .filter(|t| !self.techs.contains(&t.id) && t.after.iter().all(|a| self.techs.contains(a)))
                .collect();
            // What it studies follows what it values.
            let pick = open.iter().max_by(|a, b| self.tech_appeal(a).total_cmp(&self.tech_appeal(b)));
            self.research = pick.map(|t| t.id.clone());
        }
        if let Some(t) = self.research.clone().and_then(|r| def.techs.iter().find(|t| t.id == r))
            && self.points >= t.points {
                self.points -= t.points;
                self.techs.push(t.id.clone());
                self.research = None;
                self.note(day, format!("discovered {}.", t.name));
                events.push(TribeEvent::Discovered { tribe: self.name.clone(), tech: t.name.clone() });
            }

        // ---- learn: credit today's work with how things went
        let prosperity = food_days.min(8.0) * 0.6 + self.members.len() as f32 + self.beds as f32 * 0.3 + self.techs.len() as f32 * 0.2;
        let reward = ((prosperity - self.last_prosperity) / 3.0).clamp(-1.0, 1.0);
        let total: f32 = self.effort.iter().sum();
        if total > 0.0 && self.last_prosperity > 0.0 {
            for f in 0..FOCI {
                self.culture[f] = (self.culture[f] + LEARN_RATE * reward * self.effort[f] / total).clamp(0.2, 3.0);
            }
        }
        self.last_prosperity = prosperity;
        self.effort = [0.0; FOCI];
        self.history.push(self.members.len() as u32);
        if self.history.len() > 120 {
            self.history.remove(0);
        }

        // ---- a band leaves to settle new land
        let mut band = None;
        let people = self.members.len() as u32;
        if room && people >= def.split_at && food_days >= 1.5 {
            let n = (people / 3).max(3) as usize;
            let mut pawns = Vec::new();
            let mut members = Vec::new();
            for _ in 0..n {
                let i = rng.below(self.members.len());
                let m = self.members.remove(i);
                self.hunts.retain(|h| h.pawn != m.pawn);
                if let Some(p) = self.colony.remove_pawn(world, content, rng, m.pawn) {
                    pawns.push(p);
                    members.push(m);
                }
            }
            // They take their share of the food.
            let share = n as f32 / people as f32;
            let mut food = Vec::new();
            for (item, _) in Self::food_items(def, content) {
                let total = self.colony.items.total(item);
                let mut take = (total as f32 * share) as u32;
                let stacks: Vec<u32> = self.colony.items.stacks.values().filter(|s| s.item == item && s.reserved.is_none()).map(|s| s.id).collect();
                let mut got = 0;
                for s in stacks {
                    if take == 0 {
                        break;
                    }
                    let g = self.colony.items.take(s, take);
                    take -= g;
                    got += g;
                }
                if got > 0 {
                    food.push((item, got));
                }
            }
            self.note(day, format!("{} people left to find new land.", pawns.len()));
            band = Some(TribeEvent::Split { from: self.id, pawns, members, food });
        }
        (starved, band)
    }

    fn tech_appeal(&self, t: &TechDef) -> f32 {
        let mut v = 1.0 / t.points.max(1.0) * 30.0;
        for u in &t.unlocks {
            let f = match u.as_str() {
                "harvest" => F_FOOD,
                "chop" => F_WOOD,
                "quarry" => F_STONE,
                "hunt" => F_HUNT,
                "farm" => F_FARM,
                _ => F_BUILD,
            };
            v += self.culture[f];
        }
        v
    }

    /// The values it holds highest, for describing a culture.
    pub fn values(&self) -> Vec<&'static str> {
        let mut v: Vec<(usize, f32)> = (0..FOCI).map(|f| (f, self.culture[f])).collect();
        v.sort_by(|a, b| b.1.total_cmp(&a.1));
        v.iter().take(2).map(|(f, _)| VALUE_NAMES[*f]).collect()
    }

    /// Activity of one of its people, as shown on hover.
    pub fn activity(&self, p: &Pawn, night: bool) -> String {
        match &p.job {
            Some(j) => j.label.clone(),
            None if night => "Sleeping".into(),
            None => "Idle".into(),
        }
    }

    /// Tiles inside its finished homes.
    pub fn home_tiles(&self) -> &[TilePos] {
        &self.homes
    }

    /// Techs it knows, in the order of the def.
    pub fn known_techs<'a>(&self, def: &'a TribeDef) -> Vec<&'a TechDef> {
        def.techs.iter().filter(|t| self.techs.contains(&t.id)).collect()
    }
}

/// Distinct tiles in all of a tribe's project footprints.
pub fn footprint(t: &Tribe) -> BTreeSet<TilePos> {
    t.projects.iter().flat_map(|p| p.rect().iter().collect::<Vec<_>>()).collect()
}
