//! Wild life: animals that eat, flee, hunt, herd, breed and die on their own.
//!
//! Every creature carries **genes** (body traits like speed, size, sight,
//! cold tolerance) and a **brain** (how much it values each action). Both
//! are passed to its young with small random mutations, so traits that help
//! a lineage survive and breed spread, and the rest die out: natural
//! selection. The brain also **learns during life**: actions that paid off
//! (food found, a hunt won, a safe escape) get weighted up, ones that wasted
//! energy get weighted down, and children start from what their parents
//! learned. When a lineage drifts far enough from its ancestors it is
//! recognised and named as a **new species**.
//!
//! Everything a species is (diet, enemies, lifespan, litter size, herding,
//! cold limit, mutation rate) comes from `CreatureDef.life` data.

use std::collections::{HashMap, HashSet};

use pw_colony::{TPS, find_path};
use pw_core::*;
use pw_objects as objs;
use pw_world::*;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------- genes & brain

pub const GENES: usize = 9;
pub const G_SPEED: usize = 0;
pub const G_SIZE: usize = 1;
pub const G_SIGHT: usize = 2;
pub const G_METABOLISM: usize = 3;
pub const G_FERTILITY: usize = 4;
pub const G_BOLDNESS: usize = 5;
pub const G_COLD: usize = 6;
pub const G_LIFESPAN: usize = 7;
pub const G_HUE: usize = 8;
pub const GENE_NAMES: [&str; GENES] =
    ["speed", "size", "sight", "metabolism", "fertility", "boldness", "cold tolerance", "lifespan", "colour"];

pub const ACTIONS: usize = 7;
pub const A_EAT: u8 = 0;
pub const A_FLEE: u8 = 1;
pub const A_HUNT: u8 = 2;
pub const A_MATE: u8 = 3;
pub const A_HERD: u8 = 4;
pub const A_WANDER: u8 = 5;
pub const A_REST: u8 = 6;
pub const ACTION_NAMES: [&str; ACTIONS] = ["Eating", "Fleeing", "Hunting", "Looking for a mate", "With the herd", "Wandering", "Resting"];

/// How far a lineage must drift from its species' founders to be a new species.
const SPECIATION: f32 = 0.42;
/// How fast the brain learns from rewards.
const LEARN_RATE: f32 = 0.5;
/// Ticks between decisions.
const THINK_EVERY: u32 = 8;
/// Hard cap on all wild creatures together.
pub const MAX_CREATURES: usize = 900;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Target {
    None,
    Tile(TilePos),
    Creature(u32),
    Object(ObjId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Death {
    Starved,
    OldAge,
    Eaten,
    Burnt,
    Froze,
}

impl Death {
    pub fn describe(self) -> &'static str {
        match self {
            Death::Starved => "starved",
            Death::OldAge => "died of old age",
            Death::Eaten => "was eaten",
            Death::Burnt => "burnt",
            Death::Froze => "froze",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Creature {
    pub id: u32,
    /// Creature def index (remapped by id on load).
    pub def: usize,
    pub species: u32,
    pub pos: (f32, f32),
    /// Reversed: next step is `last()`.
    pub path: Vec<TilePos>,
    pub energy: f32,
    /// Age in ticks.
    pub age: u64,
    pub generation: u32,
    pub genes: [f32; GENES],
    pub brain: [f32; ACTIONS],
    pub action: u8,
    pub target: Target,
    /// Energy when the current action began, and a bonus/penalty collected since.
    pub action_energy: f32,
    pub action_ticks: u32,
    pub reward: f32,
    pub think: u32,
    /// Ticks until it can breed again.
    pub cooldown: u32,
    pub work: u32,
    pub food_memory: Option<TilePos>,
    pub danger: Option<TilePos>,
    pub children: u32,
    pub kills: u32,
    pub dead: Option<Death>,
}

impl Creature {
    pub fn tile(&self) -> TilePos {
        tile_of_f(self.pos.0, self.pos.1)
    }
    pub fn activity(&self) -> &'static str {
        ACTION_NAMES[self.action as usize % ACTIONS]
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Species {
    pub id: u32,
    pub def_id: String,
    pub name: String,
    /// Species it evolved from.
    pub parent: Option<u32>,
    /// Gene averages of its founders; drift is measured against these.
    pub founder: [f32; GENES],
    pub alive: u32,
    pub born: u32,
    pub died: u32,
    /// Deaths by cause: starved, old age, eaten, burnt, froze.
    #[serde(default)]
    pub causes: [u32; 5],
    pub max_generation: u32,
    pub first_tick: u64,
    pub extinct_tick: Option<u64>,
    /// Population every in-game hour (most recent last, capped).
    pub history: Vec<u32>,
}

/// What the rest of the game needs to hear about.
#[derive(Clone, Debug)]
pub enum LifeEvent {
    Died { def: usize, species: u32, pos: (f32, f32), cause: Death },
    NewSpecies { id: u32, name: String, from: String },
    Extinct { name: String },
}

/// The world as wild life sees it this tick.
#[derive(Clone, Copy, Debug)]
pub struct LifeEnv {
    pub tick: u64,
    pub day_ticks: u64,
    pub temp: f32,
    /// 0 day .. 1 night.
    pub dark: f32,
    /// Walking speed multiplier (snow).
    pub walk: f32,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Life {
    pub creatures: Vec<Creature>,
    pub next_id: u32,
    pub species: Vec<Species>,
}

/// A creature def's life data with ids resolved.
struct LifeRt {
    life: LifeDef,
    speed: f32,
    graze: Vec<(Vec<bool>, MatId, f32)>,
    browse: Vec<(Vec<String>, f32)>,
    hunt: Vec<(Vec<String>, f32)>,
}

fn resolve(content: &Content) -> Vec<Option<LifeRt>> {
    content
        .creatures
        .iter()
        .map(|(_, d)| {
            let life = d.life.clone()?;
            let mut graze = Vec::new();
            let mut browse = Vec::new();
            let mut hunt = Vec::new();
            for f in &life.diet {
                match f {
                    FoodDef::Graze { matches, leaves, energy } => {
                        if let (Some(mask), Some(m)) = (content.mat_mask(matches), content.mat(leaves)) {
                            graze.push((mask, m, *energy));
                        }
                    }
                    FoodDef::Browse { tags, energy } => browse.push((tags.clone(), *energy)),
                    FoodDef::Hunt { tags, energy } => hunt.push((tags.clone(), *energy)),
                }
            }
            Some(LifeRt { speed: d.speed, life, graze, browse, hunt })
        })
        .collect()
}

/// Rough normal noise in about -1..1.
fn gauss(rng: &mut Rng) -> f32 {
    (rng.f32() + rng.f32() + rng.f32() - 1.5) / 1.5 * 1.7
}

fn max_energy(rt: &LifeRt, g: &[f32; GENES]) -> f32 {
    100.0 * rt.life.size * g[G_SIZE]
}

fn sight(rt: &LifeRt, g: &[f32; GENES]) -> f32 {
    (rt.life.sight * g[G_SIGHT]).clamp(2.0, 40.0)
}

fn adult(rt: &LifeRt, c: &Creature, day: u64) -> bool {
    c.age as f32 >= rt.life.adult_days * day as f32
}

fn cold_limit(rt: &LifeRt, g: &[f32; GENES]) -> f32 {
    rt.life.cold_limit - (g[G_COLD] - 1.0) * 15.0
}

/// Energy burnt per tick. Speed, sight and size cost energy; efficient
/// metabolism saves it; cold costs more, and thick fur costs in the heat.
fn burn(rt: &LifeRt, c: &Creature, env: &LifeEnv, moving: bool) -> f32 {
    let g = &c.genes;
    let hour = env.day_ticks as f32 / 24.0;
    let mut b = max_energy(rt, g) * rt.life.hunger / hour;
    b *= (0.6 + 0.4 * g[G_SPEED] * g[G_SPEED]) * (0.8 + 0.2 * g[G_SIGHT]) / g[G_METABOLISM].max(0.3);
    b *= if moving { 1.25 } else if c.action == A_REST { 0.45 } else { 0.8 };
    let limit = cold_limit(rt, g);
    if env.temp < limit {
        b *= 1.0 + (limit - env.temp) * 0.12;
    }
    if env.temp > 22.0 && g[G_COLD] > 1.0 {
        b *= 1.0 + (g[G_COLD] - 1.0) * (env.temp - 22.0) * 0.05;
    }
    b
}

fn name_for(base: &str, founder: &[f32; GENES], g: &[f32; GENES]) -> String {
    // Name after the trait that changed most.
    let mut best = (0usize, 0.0f32);
    for i in 0..GENES {
        let d = g[i] - founder[i];
        if d.abs() > best.1.abs() {
            best = (i, d);
        }
    }
    let up = best.1 > 0.0;
    let word = match best.0 {
        G_SPEED => if up { "Swift" } else { "Plodding" },
        G_SIZE => if up { "Giant" } else { "Dwarf" },
        G_SIGHT => if up { "Keen-eyed" } else { "Dim-eyed" },
        G_METABOLISM => if up { "Hardy" } else { "Hungry" },
        G_FERTILITY => if up { "Fertile" } else { "Rare" },
        G_BOLDNESS => if up { "Bold" } else { "Timid" },
        G_COLD => if up { "Woolly" } else { "Sleek" },
        G_LIFESPAN => if up { "Ancient" } else { "Brief" },
        _ => if up { "Bright" } else { "Dusky" },
    };
    format!("{word} {base}")
}

fn has_any(tags: &[String], want: &[String]) -> bool {
    want.iter().any(|w| tags.contains(w))
}

/// A cheap snapshot of a creature used for perception.
#[derive(Clone, Copy)]
struct Seen {
    idx: usize,
    id: u32,
    def: usize,
    species: u32,
    pos: (f32, f32),
    ready: bool,
    dead: bool,
}

impl Life {
    pub fn alive(&self) -> usize {
        self.creatures.len()
    }

    pub fn get(&self, id: u32) -> Option<&Creature> {
        self.creatures.iter().find(|c| c.id == id)
    }

    /// Nearest creature to a world point within `reach` particles.
    pub fn at(&self, p: (f32, f32), reach: f32) -> Option<&Creature> {
        self.creatures
            .iter()
            .map(|c| (((c.pos.0 - p.0).powi(2) + (c.pos.1 - p.1).powi(2)).sqrt(), c))
            .filter(|(d, _)| *d <= reach)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|x| x.1)
    }

    pub fn species_of(&self, id: u32) -> Option<&Species> {
        self.species.iter().find(|s| s.id == id)
    }

    fn base_species(&mut self, content: &Content, def: usize, tick: u64) -> u32 {
        let def_id = content.creatures.by_idx(def).id.clone();
        if let Some(s) = self.species.iter().find(|s| s.def_id == def_id && s.parent.is_none()) {
            return s.id;
        }
        let id = self.species.len() as u32 + 1;
        self.species.push(Species {
            id,
            def_id,
            name: content.creatures.by_idx(def).name.clone(),
            parent: None,
            founder: [1.0; GENES],
            alive: 0,
            born: 0,
            died: 0,
            causes: [0; 5],
            max_generation: 0,
            first_tick: tick,
            extinct_tick: None,
            history: Vec::new(),
        });
        id
    }

    /// Spawn a grown wild creature of a def. Returns its id.
    pub fn spawn(&mut self, content: &Content, def: usize, tile: TilePos, rng: &mut Rng, tick: u64, day_ticks: u64) -> Option<u32> {
        let life = content.creatures.by_idx(def).life.as_ref()?;
        if self.creatures.len() >= MAX_CREATURES {
            return None;
        }
        let species = self.base_species(content, def, tick);
        let mut genes = [1.0; GENES];
        for g in &mut genes {
            *g = (1.0 + gauss(rng) * 0.04).max(0.3);
        }
        let mut brain = [1.0; ACTIONS];
        for b in &mut brain {
            *b = 1.0 + gauss(rng) * 0.05;
        }
        let max_e = 100.0 * life.size * genes[G_SIZE];
        let span = life.lifespan_days * day_ticks as f32;
        let adult = life.adult_days * day_ticks as f32;
        let age = (adult + rng.f32() * (span * 0.4 - adult).max(0.0)) as u64;
        let (x, y) = tile.center();
        self.next_id += 1;
        let id = self.next_id;
        self.creatures.push(Creature {
            id,
            def,
            species,
            pos: (x, y),
            path: Vec::new(),
            energy: max_e * (0.6 + rng.f32() * 0.3),
            age,
            generation: 0,
            genes,
            brain,
            action: A_WANDER,
            target: Target::None,
            action_energy: 0.0,
            action_ticks: 0,
            reward: 0.0,
            think: rng.below(THINK_EVERY as usize) as u32,
            cooldown: rng.below((day_ticks / 2) as usize) as u32,
            work: 0,
            food_memory: None,
            danger: None,
            children: 0,
            kills: 0,
            dead: None,
        });
        if let Some(s) = self.species.iter_mut().find(|s| s.id == species) {
            s.alive += 1;
        }
        Some(id)
    }

    /// Living creatures of a def (all its species together).
    pub fn count_def(&self, def: usize) -> usize {
        self.creatures.iter().filter(|c| c.def == def).count()
    }

    pub fn tick(&mut self, world: &mut World, content: &Content, rng: &mut Rng, env: &LifeEnv) -> Vec<LifeEvent> {
        let mut events = Vec::new();
        if self.creatures.is_empty() {
            return events;
        }
        let rts = resolve(content);
        let day = env.day_ticks.max(1);
        let hour = (day / 24).max(1);

        // Perception snapshot and a coarse grid (8x8 tiles per cell).
        let seen: Vec<Seen> = self
            .creatures
            .iter()
            .enumerate()
            .map(|(idx, c)| {
                let ready = rts.get(c.def).and_then(|r| r.as_ref()).is_some_and(|rt| {
                    adult(rt, c, day) && c.cooldown == 0 && c.energy > max_energy(rt, &c.genes) * 0.55
                });
                Seen { idx, id: c.id, def: c.def, species: c.species, pos: c.pos, ready, dead: c.dead.is_some() }
            })
            .collect();
        let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
        for (i, s) in seen.iter().enumerate() {
            let t = tile_of_f(s.pos.0, s.pos.1);
            grid.entry((t.x.div_euclid(8), t.y.div_euclid(8))).or_default().push(i);
        }
        let near = |p: (f32, f32), r_tiles: f32| -> Vec<usize> {
            let t = tile_of_f(p.0, p.1);
            let rc = (r_tiles / 8.0).ceil() as i32;
            let (cx, cy) = (t.x.div_euclid(8), t.y.div_euclid(8));
            let r2 = (r_tiles * TILE as f32).powi(2);
            let mut out = Vec::new();
            for gy in cy - rc..=cy + rc {
                for gx in cx - rc..=cx + rc {
                    if let Some(v) = grid.get(&(gx, gy)) {
                        for &i in v {
                            let s = &seen[i];
                            if (s.pos.0 - p.0).powi(2) + (s.pos.1 - p.1).powi(2) <= r2 {
                                out.push(i);
                            }
                        }
                    }
                }
            }
            out
        };
        let def_tags: Vec<Vec<String>> = rts.iter().map(|r| r.as_ref().map(|r| r.life.tags.clone()).unwrap_or_default()).collect();
        let pop_by_def: HashMap<usize, usize> = seen.iter().fold(HashMap::new(), |mut m, s| {
            *m.entry(s.def).or_insert(0) += 1;
            m
        });
        let id_index: HashMap<u32, usize> = seen.iter().map(|s| (s.id, s.idx)).collect();

        let mut kills: Vec<(usize, usize)> = Vec::new(); // (hunter idx, prey idx)
        let mut births: Vec<(usize, Option<usize>)> = Vec::new(); // (mother idx, father idx)
        let mut taken: HashSet<usize> = HashSet::new();

        for i in 0..self.creatures.len() {
            let Some(rt) = rts.get(self.creatures[i].def).and_then(|r| r.as_ref()) else { continue };
            let c = &mut self.creatures[i];
            if c.dead.is_some() {
                continue;
            }
            c.age += 1;
            c.cooldown = c.cooldown.saturating_sub(1);
            c.action_ticks += 1;
            let max_e = max_energy(rt, &c.genes);
            let here = c.tile();

            // ---- hazards: fire and lava burn, walls push out
            let (px, py) = (c.pos.0 as i32, c.pos.1 as i32);
            let hot = [Layer::Floor, Layer::Solid, Layer::Overlay]
                .iter()
                .any(|&l| content.mat_rt(world.get(px, py, l)).has_tag("hot"));
            if hot {
                c.energy -= max_e * 0.06;
                c.danger = Some(here);
                c.reward -= 0.3;
                c.think = 0;
                if c.energy <= 0.0 {
                    c.dead = Some(Death::Burnt);
                    continue;
                }
            }
            if world.chunk(here.chunk()).is_none() {
                continue; // frozen in unloaded land
            }
            if !world.walkable(here) {
                match pw_colony::nearest_tile(here, 3, |t| world.walkable(t)) {
                    Some(t) => {
                        let (x, y) = t.center();
                        c.pos = (x, y);
                        c.path.clear();
                    }
                    None => {
                        c.dead = Some(if hot { Death::Burnt } else { Death::Starved });
                        continue;
                    }
                }
            }

            // ---- think
            if c.think == 0 {
                c.think = THINK_EVERY;
                let s_r = sight(rt, &c.genes);
                let others = near(c.pos, s_r);
                let mut threat: Option<(f32, (f32, f32))> = None;
                let mut prey: Option<(f32, usize)> = None;
                let mut mate: Option<(f32, usize)> = None;
                let (mut kin, mut kx, mut ky) = (0usize, 0.0f32, 0.0f32);
                for &oi in &others {
                    let o = &seen[oi];
                    if o.idx == i || o.dead {
                        continue;
                    }
                    let d = ((o.pos.0 - c.pos.0).powi(2) + (o.pos.1 - c.pos.1).powi(2)).sqrt() / TILE as f32;
                    let tags = &def_tags[o.def];
                    if has_any(tags, &rt.life.flees) && threat.is_none_or(|t| d < t.0) {
                        threat = Some((d, o.pos));
                    }
                    if o.species != c.species && rt.hunt.iter().any(|(t, _)| has_any(tags, t)) && prey.is_none_or(|p| d < p.0) {
                        prey = Some((d, o.idx));
                    }
                    if o.def == c.def {
                        kin += 1;
                        kx += o.pos.0;
                        ky += o.pos.1;
                        if o.species == c.species && o.ready && mate.is_none_or(|m| d < m.0) {
                            mate = Some((d, o.idx));
                        }
                    }
                }
                // Food: remembered spot, or look around.
                let mut food: Option<Target> = None;
                if !rt.graze.is_empty() {
                    let ok = |t: TilePos| tile_food(world, &rt.graze, t).is_some();
                    if let Some(m) = c.food_memory.filter(|&t| ok(t)) {
                        food = Some(Target::Tile(m));
                    } else {
                        let r = s_r as i32;
                        let mut best: Option<(i32, TilePos)> = None;
                        for _ in 0..10 {
                            let t = here.offset(rng.range_i32(-r, r), rng.range_i32(-r, r));
                            if ok(t) && world.walkable(t) {
                                let d = t.manhattan(here);
                                if best.is_none_or(|b| d < b.0) {
                                    best = Some((d, t));
                                }
                            }
                        }
                        if let Some((_, t)) = best {
                            c.food_memory = Some(t);
                            food = Some(Target::Tile(t));
                        }
                    }
                }
                if food.is_none() && !rt.browse.is_empty() {
                    let r = s_r as i32;
                    for _ in 0..6 {
                        let t = here.offset(rng.range_i32(-r, r), rng.range_i32(-r, r));
                        if let Some(o) = world.objects_in_tile(t).into_iter().find(|&o| {
                            world.objects.get(o).is_some_and(|inst| {
                                let p = &content.protos[inst.def];
                                !inst.built && rt.browse.iter().any(|(tags, _)| has_any(&p.tags, tags))
                            })
                        }) {
                            food = Some(Target::Object(o));
                            break;
                        }
                    }
                }

                let h = 1.0 - (c.energy / max_e).clamp(0.0, 1.0);
                let g = c.genes;
                let grown = adult(rt, c, day);
                let crowded = pop_by_def.get(&c.def).copied().unwrap_or(0) >= rt.life.max_population as usize;
                let mut score = [0.0f32; ACTIONS];
                score[A_EAT as usize] = if food.is_some() { 0.15 + h * 1.3 } else { 0.0 };
                if let Some((d, _)) = prey {
                    score[A_HUNT as usize] = h * 1.2 * (0.6 + 0.4 * g[G_BOLDNESS]) * (1.0 - d / (s_r + 1.0)).max(0.2);
                }
                if let Some((d, _)) = threat {
                    score[A_FLEE as usize] = (1.0 - d / (s_r + 1.0)).max(0.0) * 1.8 * (1.6 - 0.6 * g[G_BOLDNESS]).max(0.2);
                }
                if c.danger.is_some_and(|t| t.chebyshev(here) < 4) {
                    score[A_FLEE as usize] = score[A_FLEE as usize].max(0.6);
                }
                if grown && c.cooldown == 0 && c.energy > max_e * 0.6 && !crowded {
                    let near_mate = mate.is_some() || !rt.life.needs_mate;
                    score[A_MATE as usize] = (if near_mate { 1.0 } else { 0.3 }) * g[G_FERTILITY].min(2.0) * 0.9;
                }
                if rt.life.herd > 0.0 && kin > 0 {
                    let (mx, my) = (kx / kin as f32, ky / kin as f32);
                    let d = ((mx - c.pos.0).powi(2) + (my - c.pos.1).powi(2)).sqrt() / TILE as f32;
                    score[A_HERD as usize] = rt.life.herd * (d / 4.0).min(1.0) * 0.8;
                }
                score[A_WANDER as usize] = 0.12 + if food.is_none() { h * 0.7 } else { 0.0 };
                score[A_REST as usize] = (0.08 + env.dark * 0.45) * (1.0 - h);
                let mut best = (A_WANDER, f32::MIN);
                for a in 0..ACTIONS {
                    let mut v = score[a] * c.brain[a];
                    if v <= 0.0 {
                        continue;
                    }
                    if a as u8 == c.action {
                        v += 0.05; // a little persistence
                    }
                    v += rng.f32() * 0.04; // a little curiosity
                    if v > best.1 {
                        best = (a as u8, v);
                    }
                }
                let next = best.0;
                // Learn from how the last action went: energy compared to just sitting still,
                // plus anything notable (a birth, a kill, getting hurt).
                if next != c.action || c.action_ticks > 240 {
                    let idle = burn(rt, c, env, false) * c.action_ticks as f32;
                    let r = (c.energy - c.action_energy + idle) / max_e + c.reward;
                    let a = c.action as usize;
                    c.brain[a] = (c.brain[a] + LEARN_RATE * r).clamp(0.2, 3.0);
                    c.action = next;
                    c.action_energy = c.energy;
                    c.action_ticks = 0;
                    c.reward = 0.0;
                    c.path.clear();
                }
                c.target = match next {
                    A_EAT => food.unwrap_or(Target::None),
                    A_HUNT => prey.map(|p| Target::Creature(seen[p.1].id)).unwrap_or(Target::None),
                    A_MATE => mate.map(|m| Target::Creature(seen[m.1].id)).unwrap_or(Target::None),
                    A_FLEE => {
                        let from = threat
                            .map(|t| t.1)
                            .or(c.danger.map(|t| t.center()))
                            .unwrap_or((c.pos.0 + 1.0, c.pos.1));
                        let (dx, dy) = (c.pos.0 - from.0, c.pos.1 - from.1);
                        let l = (dx * dx + dy * dy).sqrt().max(0.01);
                        let far = (c.pos.0 + dx / l * 7.0 * TILE as f32, c.pos.1 + dy / l * 7.0 * TILE as f32);
                        Target::Tile(tile_of_f(far.0, far.1))
                    }
                    A_HERD if kin > 0 => Target::Tile(tile_of_f(kx / kin as f32, ky / kin as f32)),
                    A_WANDER => Target::Tile(here.offset(rng.range_i32(-6, 6), rng.range_i32(-6, 6))),
                    _ => Target::None,
                };
                if c.danger.is_some() && rng.chance(0.02) {
                    c.danger = None; // fear fades
                }
                if matches!(c.target, Target::Tile(_) | Target::Object(_)) || next == A_HUNT || next == A_MATE {
                    c.path.clear();
                }
            }
            c.think -= 1;

            // ---- move toward the target
            let goal = match c.target {
                Target::Tile(t) => Some(t),
                Target::Object(o) => world.objects.get(o).map(|i| i.origin_tile()),
                Target::Creature(id) => id_index.get(&id).map(|&k| tile_of_f(seen[k].pos.0, seen[k].pos.1)),
                Target::None => None,
            };
            let mut moving = false;
            if let Some(goal) = goal {
                // Food and places: stand on the tile. Creatures and plants: stand next to them.
                let reach = if matches!(c.target, Target::Tile(_)) { 0 } else { 1 };
                let close = goal.chebyshev(here) <= reach;
                if !close {
                    if c.path.is_empty() || c.action_ticks % 24 == 0 && c.path.first().is_none_or(|e| e.chebyshev(goal) > reach) {
                        c.path = find_path(world, here, |t| t.chebyshev(goal) <= reach, goal, 260).unwrap_or_default();
                        if c.path.is_empty() {
                            // No path: give up on this target for now.
                            c.target = Target::None;
                            c.think = 0;
                        }
                    }
                    let speed = rt.speed * c.genes[G_SPEED] * env.walk;
                    moving = step(c, world, speed);
                }
            }

            // ---- act
            c.work += 1;
            match (c.action, c.target) {
                (A_EAT, Target::Tile(t)) if t == c.tile() && c.work % 10 == 0 => {
                    let mut ate = false;
                    if let Some((mask, leaves, e)) = tile_food(world, &rt.graze, t) {
                        // Nibble the nearest bits around the mouth, a little at random,
                        // so grazed patches come out round and ragged, not square.
                        let (mx, my) = (c.pos.0 + rng.range_f32(-1.5, 1.5), c.pos.1 + rng.range_f32(-1.5, 1.5));
                        let mut bites: Vec<(f32, i32, i32)> = Vec::new();
                        let r = 5;
                        for dy in -r..=r {
                            for dx in -r..=r {
                                let (x, y) = (mx as i32 + dx, my as i32 + dy);
                                let d = ((x as f32 + 0.5 - mx).powi(2) + (y as f32 + 0.5 - my).powi(2)).sqrt();
                                if d > r as f32 {
                                    continue;
                                }
                                let m = world.get(x, y, Layer::Floor);
                                if mask.get(m as usize).copied().unwrap_or(false) {
                                    bites.push((d + rng.f32() * 1.5, x, y));
                                }
                            }
                        }
                        bites.sort_by(|a, b| a.0.total_cmp(&b.0));
                        for &(_, x, y) in bites.iter().take(3) {
                            world.set(content, x, y, Layer::Floor, leaves, NO_OBJ);
                            c.energy = (c.energy + e).min(max_e);
                            ate = true;
                        }
                    }
                    if !ate {
                        c.food_memory = None;
                        c.think = 0;
                    }
                }
                (A_EAT, Target::Object(o)) if c.work % 15 == 0 => {
                    if let Some(inst) = world.objects.get(o).cloned() {
                        if inst.origin_tile().chebyshev(c.tile()) <= 1 {
                            let e = rt.browse.first().map(|b| b.1).unwrap_or(1.0);
                            if objs::pluck(world, content, o, rng).is_some() {
                                c.energy = (c.energy + e * 2.0).min(max_e);
                            } else if objs::chip(world, content, o, rng, Some(Layer::Overlay)).is_some() {
                                c.energy = (c.energy + e).min(max_e);
                            } else {
                                c.think = 0;
                            }
                        }
                    } else {
                        c.think = 0;
                    }
                }
                (A_HUNT, Target::Creature(id)) if c.work % 10 == 0 => {
                    if let Some(&pi) = id_index.get(&id) {
                        let p = seen[pi];
                        let d = ((p.pos.0 - c.pos.0).powi(2) + (p.pos.1 - c.pos.1).powi(2)).sqrt();
                        if d <= TILE as f32 * 1.7 && !p.dead && !taken.contains(&pi) {
                            kills.push((i, pi));
                            taken.insert(pi);
                        }
                    }
                }
                (A_MATE, _) if c.cooldown == 0 && adult(rt, c, day) && c.energy > max_e * 0.55 => {
                    if !rt.life.needs_mate {
                        births.push((i, None));
                        c.cooldown = u32::MAX; // set properly when the birth happens
                    } else if let Target::Creature(id) = c.target {
                        if let Some(&mi) = id_index.get(&id) {
                            let m = seen[mi];
                            let d = ((m.pos.0 - c.pos.0).powi(2) + (m.pos.1 - c.pos.1).powi(2)).sqrt();
                            if d <= TILE as f32 * 1.6 && m.ready && !taken.contains(&mi) && !taken.contains(&i) {
                                births.push((i, Some(mi)));
                                taken.insert(i);
                                taken.insert(mi);
                            }
                        }
                    }
                }
                _ => {}
            }

            // ---- life and death
            let b = burn(rt, c, env, moving);
            c.energy -= b;
            if c.energy <= 0.0 {
                c.dead = Some(if env.temp < cold_limit(rt, &c.genes) - 5.0 { Death::Froze } else { Death::Starved });
            } else if c.age as f32 > rt.life.lifespan_days * c.genes[G_LIFESPAN] * day as f32 {
                c.dead = Some(Death::OldAge);
            }
        }

        // ---- hunts: the stronger, bolder, bigger hunter tends to win; fast prey escape
        for (hi, pi) in kills {
            let (Some(hrt), Some(prt)) = (
                rts.get(self.creatures[hi].def).and_then(|r| r.as_ref()),
                rts.get(self.creatures[pi].def).and_then(|r| r.as_ref()),
            ) else {
                continue;
            };
            if self.creatures[pi].dead.is_some() || self.creatures[hi].dead.is_some() {
                continue;
            }
            let hg = self.creatures[hi].genes;
            let pg = self.creatures[pi].genes;
            let power = hrt.life.size * hg[G_SIZE] * (0.6 + 0.4 * hg[G_BOLDNESS]) * hg[G_SPEED];
            let resist = prt.life.size * pg[G_SIZE] * (0.4 + 0.6 * pg[G_SPEED]) + 0.2;
            let win = (0.35 * power / resist).clamp(0.03, 0.9);
            if rng.chance(win) {
                let prey_energy = self.creatures[pi].energy.max(0.0) + max_energy(prt, &pg) * 0.3;
                self.creatures[pi].dead = Some(Death::Eaten);
                let share = hrt.hunt.first().map(|h| h.1).unwrap_or(0.6);
                let h = &mut self.creatures[hi];
                h.energy = (h.energy + prey_energy * share).min(max_energy(hrt, &hg));
                h.kills += 1;
                h.reward += 0.3;
            } else {
                // The prey got away and learned to fear this spot.
                let hpos = self.creatures[hi].tile();
                let p = &mut self.creatures[pi];
                p.danger = Some(hpos);
                p.reward -= 0.2;
                p.think = 0;
            }
        }

        // ---- births
        for (mi, fi) in births {
            let Some(rt) = rts.get(self.creatures[mi].def).and_then(|r| r.as_ref()) else { continue };
            let mother = self.creatures[mi].clone();
            let father = fi.map(|f| self.creatures[f].clone());
            if mother.dead.is_some() || father.as_ref().is_some_and(|f| f.dead.is_some()) {
                continue;
            }
            if self.count_def(mother.def) >= rt.life.max_population as usize || self.creatures.len() >= MAX_CREATURES {
                self.creatures[mi].cooldown = (day / 4) as u32;
                continue;
            }
            let (lo, hi) = rt.life.litter;
            let base = rng.range_i32(lo as i32, hi.max(lo) as i32) as f32;
            let n = ((base * mother.genes[G_FERTILITY] / mother.genes[G_METABOLISM].max(0.5).sqrt()).round() as u32).clamp(1, 8);
            let mut born = 0;
            for _ in 0..n {
                let mut genes = [1.0f32; GENES];
                for k in 0..GENES {
                    let from = match &father {
                        Some(f) if rng.chance(0.5) => f.genes[k],
                        _ => mother.genes[k],
                    };
                    genes[k] = (from + gauss(rng) * rt.life.mutation).clamp(0.3, 3.0);
                }
                let mut brain = [1.0f32; ACTIONS];
                for k in 0..ACTIONS {
                    let avg = match &father {
                        Some(f) => (mother.brain[k] + f.brain[k]) * 0.5,
                        None => mother.brain[k],
                    };
                    // Children start from what their parents learned, plus a little of their own.
                    brain[k] = (avg + gauss(rng) * 0.08).clamp(0.2, 3.0);
                }
                let generation = mother.generation.max(father.as_ref().map(|f| f.generation).unwrap_or(0)) + 1;
                // Has this lineage become something new?
                let mut species = mother.species;
                if let Some(sp) = self.species.iter().find(|s| s.id == species) {
                    let drift = (0..GENES).map(|k| (genes[k] - sp.founder[k]).powi(2)).sum::<f32>().sqrt();
                    if drift > SPECIATION {
                        let base = content.creatures.by_idx(mother.def).name.clone();
                        let name = name_for(&base, &sp.founder, &genes);
                        let from = sp.name.clone();
                        let id = self.species.len() as u32 + 1;
                        self.species.push(Species {
                            id,
                            def_id: sp.def_id.clone(),
                            name: name.clone(),
                            parent: Some(sp.id),
                            founder: genes,
                            alive: 0,
                            born: 0,
                            died: 0,
                            causes: [0; 5],
                            max_generation: generation,
                            first_tick: env.tick,
                            extinct_tick: None,
                            history: Vec::new(),
                        });
                        events.push(LifeEvent::NewSpecies { id, name, from });
                        species = id;
                    }
                }
                let max_e = max_energy(rt, &genes);
                self.next_id += 1;
                let id = self.next_id;
                self.creatures.push(Creature {
                    id,
                    def: mother.def,
                    species,
                    pos: mother.pos,
                    path: Vec::new(),
                    energy: max_e * 0.5,
                    age: 0,
                    generation,
                    genes,
                    brain,
                    action: A_HERD,
                    target: Target::None,
                    action_energy: max_e * 0.5,
                    action_ticks: 0,
                    reward: 0.0,
                    think: rng.below(THINK_EVERY as usize) as u32,
                    cooldown: 0,
                    work: 0,
                    food_memory: mother.food_memory,
                    danger: None,
                    children: 0,
                    kills: 0,
                    dead: None,
                });
                if let Some(s) = self.species.iter_mut().find(|s| s.id == species) {
                    s.born += 1;
                    s.max_generation = s.max_generation.max(generation);
                }
                born += 1;
            }
            let wait = (rt.life.breed_every_hours * hour as f32 * mother.genes[G_LIFESPAN].sqrt() / mother.genes[G_FERTILITY].max(0.3)) as u32;
            let m = &mut self.creatures[mi];
            m.energy -= max_energy(rt, &m.genes) * 0.15 * born as f32;
            m.cooldown = wait.max(60);
            m.children += born;
            m.reward += 0.5;
            if let Some(f) = fi {
                let f = &mut self.creatures[f];
                f.cooldown = wait.max(60);
                f.energy -= max_energy(rt, &f.genes) * 0.05;
                f.children += born;
                f.reward += 0.5;
            }
        }

        // ---- remove the dead
        let mut keep = Vec::with_capacity(self.creatures.len());
        for c in std::mem::take(&mut self.creatures) {
            match c.dead {
                Some(cause) => {
                    if let Some(s) = self.species.iter_mut().find(|s| s.id == c.species) {
                        s.died += 1;
                        s.causes[cause as usize] += 1;
                    }
                    events.push(LifeEvent::Died { def: c.def, species: c.species, pos: c.pos, cause });
                }
                None => keep.push(c),
            }
        }
        self.creatures = keep;

        // ---- census
        let mut alive: HashMap<u32, u32> = HashMap::new();
        for c in &self.creatures {
            *alive.entry(c.species).or_insert(0) += 1;
        }
        for s in &mut self.species {
            let now = alive.get(&s.id).copied().unwrap_or(0);
            if s.alive > 0 && now == 0 && s.extinct_tick.is_none() {
                s.extinct_tick = Some(env.tick);
                events.push(LifeEvent::Extinct { name: s.name.clone() });
            }
            if now > 0 {
                s.extinct_tick = None;
            }
            s.alive = now;
            if env.tick % hour == 0 {
                s.history.push(now);
                if s.history.len() > 240 {
                    s.history.remove(0);
                }
            }
        }
        events
    }
}

/// Grazing food on a tile: (mask, what's left behind, energy per particle).
fn tile_food<'a>(world: &World, graze: &'a [(Vec<bool>, MatId, f32)], t: TilePos) -> Option<(&'a Vec<bool>, MatId, f32)> {
    for (mask, leaves, e) in graze {
        let (x, y) = t.origin();
        // Sample a few cells; grazing leaves patches, so "some" is enough.
        let hits = [(1, 1), (6, 2), (3, 5), (5, 6), (2, 3)]
            .iter()
            .filter(|&&(dx, dy)| mask.get(world.get(x + dx, y + dy, Layer::Floor) as usize).copied().unwrap_or(false))
            .count();
        if hits >= 2 {
            return Some((mask, *leaves, *e));
        }
    }
    None
}

/// Walk one tick along the path. Returns true if it moved.
fn step(c: &mut Creature, world: &World, speed_tiles: f32) -> bool {
    let Some(&next) = c.path.last() else { return false };
    let Some(info) = world.tile(next) else {
        c.path.clear();
        return false;
    };
    if !info.walkable {
        c.path.clear();
        return false;
    }
    let mut budget = speed_tiles * TILE as f32 / TPS / info.cost.max(0.2);
    while budget > 0.0 {
        let Some(&n) = c.path.last() else { break };
        let (tx, ty) = n.center();
        let (dx, dy) = (tx - c.pos.0, ty - c.pos.1);
        let d = (dx * dx + dy * dy).sqrt();
        if d <= budget {
            c.pos = (tx, ty);
            budget -= d;
            c.path.pop();
        } else {
            c.pos.0 += dx / d * budget;
            c.pos.1 += dy / d * budget;
            budget = 0.0;
        }
    }
    true
}
