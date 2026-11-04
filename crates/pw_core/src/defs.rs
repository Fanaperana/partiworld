//! Data definitions. These are what mod files (RON) deserialize into.
//! References to other content are plain string ids like `"core:oak_wood"`;
//! `Content::link` resolves them into numeric ids.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::registry::Def;

fn one() -> f32 {
    1.0
}
fn half() -> f32 {
    0.5
}
fn full_range() -> (f32, f32) {
    (0.0, 1.0)
}
fn default_stack() -> u32 {
    75
}
fn default_priority() -> i32 {
    50
}
fn yes() -> bool {
    true
}

/// The three particle layers of a cell.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default,
)]
pub enum Layer {
    /// Ground: grass, dirt, sand, water, wooden floor.
    Floor,
    /// Things that stand on the ground: trunks, rock, walls. May block movement.
    #[default]
    Solid,
    /// Above everything: canopies, roofs. Never blocks; drawn over colonists.
    Overlay,
}

impl Layer {
    pub const ALL: [Layer; 3] = [Layer::Floor, Layer::Solid, Layer::Overlay];
    pub fn index(self) -> usize {
        self as usize
    }
    pub fn from_index(i: usize) -> Layer {
        Layer::ALL[i.min(2)]
    }
    pub fn parse(s: &str) -> Option<Layer> {
        match s.to_ascii_lowercase().as_str() {
            "floor" => Some(Layer::Floor),
            "solid" => Some(Layer::Solid),
            "overlay" => Some(Layer::Overlay),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Layer::Floor => "floor",
            Layer::Solid => "solid",
            Layer::Overlay => "overlay",
        }
    }
}

/// `mod.toml`
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModInfo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    /// Mods that must be present and load before this one.
    #[serde(default)]
    pub depends: Vec<String>,
    /// Optional ordering hint: load after these if they exist.
    #[serde(default)]
    pub load_after: Vec<String>,
    #[serde(skip)]
    pub path: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct YieldDef {
    pub item: String,
    /// One item per this many particles removed.
    pub per: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MaterialDef {
    pub id: String,
    pub name: String,
    /// `#rrggbb` or `#rrggbbaa`.
    pub color: String,
    /// 0..1, how much each particle's shade may differ.
    #[serde(default)]
    pub variance: f32,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Blocks movement when placed in the solid layer.
    #[serde(default)]
    pub blocks: bool,
    /// Movement cost when used as floor. 0 = impassable (deep water).
    #[serde(default = "one")]
    pub walk_cost: f32,
    /// Work multiplier for mining/chopping.
    #[serde(default = "one")]
    pub hardness: f32,
    #[serde(default)]
    pub yields: Option<YieldDef>,
    /// Warmth it gives off, C added to tiles close by (fire, embers, lava).
    #[serde(default)]
    pub heat: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct SpriteDef {
    /// char -> `#rrggbb[aa]` or a slot like `$shirt` filled per instance.
    pub palette: BTreeMap<char, String>,
    /// Top row first.
    pub rows: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ItemDef {
    pub id: String,
    pub name: String,
    pub color: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "default_stack")]
    pub stack: u32,
    #[serde(default)]
    pub sprite: Option<SpriteDef>,
    /// How much of an empty stomach one item fills (0 = not food).
    #[serde(default)]
    pub nutrition: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PatternLayer {
    pub layer: Layer,
    /// Top row first. ' ' and '.' are empty.
    pub rows: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum Primitive {
    Circle {
        layer: Layer,
        material: String,
        radius: (f32, f32),
        #[serde(default)]
        offset: (f32, f32),
        #[serde(default = "one")]
        density: f32,
    },
    /// Circle with a wobbly, noisy edge.
    Blob {
        layer: Layer,
        material: String,
        radius: (f32, f32),
        #[serde(default)]
        offset: (f32, f32),
        #[serde(default = "half")]
        roughness: f32,
        #[serde(default = "one")]
        density: f32,
    },
    Rect {
        layer: Layer,
        material: String,
        size: (i32, i32),
        #[serde(default)]
        offset: (f32, f32),
        #[serde(default = "one")]
        density: f32,
    },
    /// A number of single particles thrown inside a radius (berries, flowers).
    Scatter {
        layer: Layer,
        material: String,
        radius: f32,
        count: (u32, u32),
        #[serde(default)]
        offset: (f32, f32),
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum ShapeDef {
    /// Hand-painted particle grid (the in-game editor writes these).
    Pattern {
        palette: BTreeMap<char, String>,
        layers: Vec<PatternLayer>,
    },
    /// Built from primitives with a per-instance seed: every tree looks different.
    Procedural(Vec<Primitive>),
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum Anchor {
    /// Origin is the centre particle (trees, rocks).
    #[default]
    Center,
    /// Origin is the bottom-left particle of a tile (walls, floors).
    Tile,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DropDef {
    pub item: String,
    pub min: u32,
    pub max: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BuildDef {
    /// (item id, count)
    #[serde(default)]
    pub cost: Vec<(String, u32)>,
    /// Total work ticks.
    pub work: u32,
    #[serde(default)]
    pub category: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GrowthDef {
    /// Object this becomes when grown.
    pub next: String,
    pub ticks: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegrowDef {
    /// Materials from the shape that grow back (e.g. berries).
    pub materials: Vec<String>,
    /// Check every N ticks.
    pub interval: u32,
    /// Chance per missing particle per check.
    pub chance: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ObjectDef {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub shape: ShapeDef,
    #[serde(default)]
    pub anchor: Anchor,
    /// Object breaks once this fraction of its particles has been removed.
    #[serde(default = "half")]
    pub break_fraction: f32,
    #[serde(default)]
    pub drops: Vec<DropDef>,
    #[serde(default)]
    pub build: Option<BuildDef>,
    #[serde(default)]
    pub growth: Option<GrowthDef>,
    #[serde(default)]
    pub regrow: Option<RegrowDef>,
    /// Rhai hooks: `on_destroyed`, `on_built`, `on_harvested`, `on_grown`.
    #[serde(default)]
    pub hooks: BTreeMap<String, String>,
    /// Plants that seed new plants nearby.
    #[serde(default)]
    pub spread: Option<SpreadDef>,
    /// Roofs: must be within this many tiles of a support (a wall or rock,
    /// i.e. solid particles tagged `support`), or it can't be built and it
    /// collapses. 0 = needs no support.
    #[serde(default)]
    pub needs_support: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VeinDef {
    pub material: String,
    /// Noise threshold in 0..1, higher = rarer.
    pub threshold: f32,
    pub frequency: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScatterDef {
    pub object: String,
    /// Grid spacing in tiles; at most one object per cell of this size.
    pub spacing: u32,
    /// Chance a grid cell gets the object.
    pub chance: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BiomeDef {
    pub id: String,
    pub name: String,
    /// Higher priority biomes are tested first.
    #[serde(default)]
    pub priority: i32,
    #[serde(default = "full_range")]
    pub elevation: (f32, f32),
    #[serde(default = "full_range")]
    pub moisture: (f32, f32),
    #[serde(default = "full_range")]
    pub temperature: (f32, f32),
    /// Weighted floor materials, blended with detail noise.
    pub floor: Vec<(String, f32)>,
    /// Fill the solid layer (mountains).
    #[serde(default)]
    pub solid: Option<String>,
    #[serde(default)]
    pub veins: Vec<VeinDef>,
    #[serde(default)]
    pub scatter: Vec<ScatterDef>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum ReactionMode {
    /// Runs every tick on cells holding an "active" material (fire).
    #[default]
    Active,
    /// Runs on randomly sampled cells (slow growth, decay).
    Random,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Target {
    /// The reacting cell.
    Me,
    /// The matched neighbour.
    Other,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NeighborDef {
    pub layer: Layer,
    /// Material id, `tag:<tag>`, `none` or `any`.
    pub matches: String,
    /// Also consider the same cell (in another layer).
    #[serde(default)]
    pub same_cell: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EffectDef {
    pub target: Target,
    /// Defaults to the target's layer.
    #[serde(default)]
    pub layer: Option<Layer>,
    /// Material id or `none`.
    pub set: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReactionDef {
    pub id: String,
    pub layer: Layer,
    /// Material id or `tag:<tag>`.
    pub input: String,
    #[serde(default)]
    pub neighbor: Option<NeighborDef>,
    pub chance: f32,
    #[serde(default)]
    pub mode: ReactionMode,
    pub effects: Vec<EffectDef>,
    /// Chance multiplier when everything is soaked (rain). 1 = weather does not matter.
    #[serde(default = "one")]
    pub wet: f32,
    /// The neighbour is picked downwind more often in wind (fire, spores).
    #[serde(default)]
    pub windy: bool,
    /// Move instead of transform: this cell and the matched neighbour swap
    /// places (smoke drifting, sand sliding). Effects then apply to the new
    /// positions: `Me` = where the neighbour's material ended up.
    #[serde(default)]
    pub swap: bool,
    /// Only happens within this outdoor temperature range (degrees C).
    #[serde(default)]
    pub min_temp: Option<f32>,
    #[serde(default)]
    pub max_temp: Option<f32>,
}

/// Built-in behaviours a job can use. New job kinds are made by pointing a
/// `JobDef` at a driver with different target tags.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum JobDriver {
    /// Wear an object down particle by particle until it breaks (chop, cut).
    Fell,
    /// Remove solid terrain particles from a tile (mine).
    Mine,
    /// Pick the yield particles off an object, leaving it standing (berries).
    Harvest,
    /// Take a built object apart.
    Deconstruct,
    /// Place a growing object (sapling, crop).
    Plant,
    /// Carry items to stockpiles and blueprints.
    Haul,
    /// Build blueprints.
    Construct,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobDef {
    pub id: String,
    pub label: String,
    pub driver: JobDriver,
    /// Object tags (Fell/Harvest/Deconstruct) or material tags (Mine).
    #[serde(default)]
    pub targets: Vec<String>,
    #[serde(default = "default_priority")]
    pub priority: i32,
    /// Designation marker colour.
    #[serde(default)]
    pub color: String,
    /// Work ticks per particle.
    #[serde(default = "one")]
    pub speed: f32,
    /// Shown as a designation tool.
    #[serde(default = "yes")]
    pub designatable: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreatureDef {
    pub id: String,
    pub name: String,
    pub sprite: SpriteDef,
    /// Tiles per second.
    pub speed: f32,
    #[serde(default)]
    pub names: Vec<String>,
    /// Colours for `$slot` palette entries, one is picked per creature.
    #[serde(default)]
    pub slots: BTreeMap<String, Vec<String>>,
    /// Wild life: present for animals that live, eat, breed, learn and evolve
    /// on their own. Colonists leave this out.
    #[serde(default)]
    pub life: Option<LifeDef>,
    /// Colonists: hunger, rest and warmth. Left out, the creature has no needs.
    #[serde(default)]
    pub needs: Option<NeedsDef>,
}

/// What a colonist needs to stay alive and well.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NeedsDef {
    /// In-game days a full stomach lasts.
    #[serde(default = "one")]
    pub food_days: f32,
    /// In-game days a rested colonist can stay awake.
    #[serde(default = "default_awake")]
    pub awake_days: f32,
    /// Comfortable temperature range, C. Colder than this chills them.
    #[serde(default = "default_comfort")]
    pub comfort: (f32, f32),
    /// Days of starving or freezing it takes to die.
    #[serde(default = "one")]
    pub toughness: f32,
}

fn default_awake() -> f32 {
    0.75
}
fn default_comfort() -> (f32, f32) {
    (12.0, 32.0)
}

/// How a wild creature lives. Genes start at 1.0 (the species average) and
/// drift through mutation; `brain` weights are learned and passed on.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LifeDef {
    /// Tags other creatures use to recognise this one (prey, predator...).
    #[serde(default)]
    pub tags: Vec<String>,
    /// What it eats.
    pub diet: Vec<FoodDef>,
    /// Creatures with these tags are feared and fled from.
    #[serde(default)]
    pub flees: Vec<String>,
    /// Body size (1 = colonist-sized). Bigger needs more food, wins fights.
    #[serde(default = "one")]
    pub size: f32,
    /// Sight in tiles.
    #[serde(default = "default_sight")]
    pub sight: f32,
    /// Energy used per in-game hour, as a fraction of a full stomach.
    #[serde(default = "default_hunger")]
    pub hunger: f32,
    /// Lifespan and adulthood, in in-game days.
    pub lifespan_days: f32,
    #[serde(default = "default_adult")]
    pub adult_days: f32,
    /// Young per birth (min, max) and hours between births.
    #[serde(default = "default_litter")]
    pub litter: (u32, u32),
    #[serde(default = "default_breed_hours")]
    pub breed_every_hours: f32,
    /// Needs a partner of the same species nearby to breed.
    #[serde(default = "yes")]
    pub needs_mate: bool,
    /// 0..1 how much it wants to stay near its own kind (herds, packs).
    #[serde(default)]
    pub herd: f32,
    /// Below this outdoor temperature (C) it starts freezing; genes shift it.
    #[serde(default)]
    pub cold_limit: f32,
    /// How strongly genes change from parent to child (0.05 = gently).
    #[serde(default = "default_mutation")]
    pub mutation: f32,
    /// Most of this species alive at once (keeps the simulation fast).
    #[serde(default = "default_max_pop")]
    pub max_population: u32,
    /// Wanders back in from the wilds if the local population dies out.
    #[serde(default)]
    pub migrates: bool,
    /// Items left behind when it dies (not when burnt).
    #[serde(default)]
    pub remains: Vec<(String, u32)>,
}

fn default_sight() -> f32 {
    10.0
}
fn default_hunger() -> f32 {
    0.03
}
fn default_adult() -> f32 {
    2.0
}
fn default_litter() -> (u32, u32) {
    (1, 2)
}
fn default_breed_hours() -> f32 {
    24.0
}
fn default_mutation() -> f32 {
    0.06
}
fn default_max_pop() -> u32 {
    120
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum FoodDef {
    /// Eat floor particles (`matches`: id, `tag:x`) and leave `leaves` behind.
    Graze {
        matches: String,
        leaves: String,
        energy: f32,
    },
    /// Eat from objects with these tags: their yield (berries) first, then leaves.
    Browse { tags: Vec<String>, energy: f32 },
    /// Hunt creatures with these tags; gains this share of the prey's energy.
    Hunt { tags: Vec<String>, energy: f32 },
}

/// Plants that spread: a grown plant drops this object nearby now and then.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpreadDef {
    pub object: String,
    /// Chance per check (scaled by the season's growth).
    pub chance: f32,
    /// How far, in tiles.
    pub radius: u32,
    /// Ticks between checks.
    pub interval: u32,
    /// Skip if this many objects already stand within the radius (no overcrowding).
    #[serde(default = "default_crowd")]
    pub max_nearby: u32,
}

fn default_crowd() -> u32 {
    6
}

/// Seasons, day length, temperature and weather. Lives in a mod's `climate.ron`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClimateDef {
    pub days_per_season: u32,
    pub seasons: Vec<SeasonDef>,
    pub weather: Vec<WeatherDef>,
    /// How much a biome's temperature noise (0..1) shifts the local temperature, +/- this.
    #[serde(default = "biome_temp_range")]
    pub biome_temp_range: f32,
}

fn biome_temp_range() -> f32 {
    8.0
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SeasonDef {
    pub name: String,
    /// Mean temperature (C) and how far it swings warmer by day / colder at night.
    pub temp: f32,
    pub swing: f32,
    /// Hours of sunrise and sunset (dawn and dusk last about an hour each side).
    pub sunrise: f32,
    pub sunset: f32,
    /// Plant growth speed multiplier (0 = no growth).
    #[serde(default = "one")]
    pub growth: f32,
    /// 0 = green leaves, 1 = full autumn colours.
    #[serde(default)]
    pub autumn: f32,
    /// Fraction of leaves gone from trees that are not `evergreen`.
    #[serde(default)]
    pub leaf_drop: f32,
    /// Weather id -> relative weight.
    pub weather: BTreeMap<String, f32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WeatherDef {
    pub id: String,
    pub name: String,
    /// 0..1. Falls as snow below freezing, rain above.
    #[serde(default)]
    pub precipitation: f32,
    /// Wind strength range, 0..1.
    #[serde(default)]
    pub wind: (f32, f32),
    /// 0..1 cloud cover (dims the light) and fog.
    #[serde(default)]
    pub clouds: f32,
    #[serde(default)]
    pub fog: f32,
    /// Temperature offset (C).
    #[serde(default)]
    pub temp: f32,
    /// How long it lasts, in hours (min, max).
    pub hours: (f32, f32),
}

impl Default for ClimateDef {
    fn default() -> Self {
        let mut w = BTreeMap::new();
        w.insert("clear".to_string(), 1.0);
        Self {
            days_per_season: 5,
            seasons: vec![SeasonDef {
                name: "Summer".into(),
                temp: 18.0,
                swing: 6.0,
                sunrise: 6.0,
                sunset: 20.0,
                growth: 1.0,
                autumn: 0.0,
                leaf_drop: 0.0,
                weather: w,
            }],
            weather: vec![WeatherDef {
                id: "clear".into(),
                name: "Clear".into(),
                precipitation: 0.0,
                wind: (0.0, 0.3),
                clouds: 0.0,
                fog: 0.0,
                temp: 0.0,
                hours: (6.0, 12.0),
            }],
            biome_temp_range: 8.0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorldGenDef {
    pub elevation_frequency: f32,
    pub moisture_frequency: f32,
    pub temperature_frequency: f32,
    pub detail_frequency: f32,
    /// Domain-warp strength in particles; makes biome edges organic.
    pub warp: f32,
}

impl Default for WorldGenDef {
    fn default() -> Self {
        Self {
            elevation_frequency: 0.0012,
            moisture_frequency: 0.0009,
            temperature_frequency: 0.0005,
            detail_frequency: 0.06,
            warp: 40.0,
        }
    }
}

macro_rules! impl_def {
    ($($t:ty),*) => { $(impl Def for $t { fn def_id(&self) -> &str { &self.id } })* };
}
impl_def!(
    MaterialDef,
    ItemDef,
    ObjectDef,
    BiomeDef,
    ReactionDef,
    JobDef,
    CreatureDef,
    PowerDef
);

/// A god power: a brush the player paints onto the world, made of effects.
/// Mods add powers by adding files to `powers/`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PowerDef {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Groups powers in the menu.
    #[serde(default = "default_power_category")]
    pub category: String,
    /// Starting brush radius in particles.
    #[serde(default = "default_power_radius")]
    pub radius: i32,
    /// Keep applying while the mouse is held (painting) instead of once per click.
    #[serde(default)]
    pub continuous: bool,
    pub effects: Vec<PowerEffect>,
}

fn default_power_category() -> String {
    "Powers".into()
}
fn default_power_radius() -> i32 {
    6
}
fn full_ring() -> (f32, f32) {
    (0.0, 1.0)
}

/// One thing a power does. `ring` limits it to part of the brush, as
/// fractions of the radius (0 = centre, 1 = edge; may go past 1).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum PowerEffect {
    /// Set particles of a layer. `onto` limits which ones (material id,
    /// `tag:x`, `none` or `any`). Setting particles of objects damages them.
    Paint {
        layer: Layer,
        set: String,
        #[serde(default = "one")]
        chance: f32,
        #[serde(default)]
        onto: Option<String>,
        #[serde(default = "full_ring")]
        ring: (f32, f32),
    },
    /// Set flammable things on fire.
    Ignite {
        #[serde(default = "one")]
        chance: f32,
        #[serde(default = "full_ring")]
        ring: (f32, f32),
    },
    /// Drop objects at random spots inside the brush.
    Place { object: String, count: u32 },
    /// Spawn creatures inside the brush.
    Spawn { creature: String, count: u32 },
    /// Change the weather right away.
    Weather { id: String },
    /// Call a script function `fn name(ctx, x, y)`.
    Script { function: String },
    /// A bright flash (lightning, explosions); purely visual.
    Flash,
    /// Found a new tribe of this kind here.
    Tribe {
        tribe: String,
        #[serde(default)]
        members: u32,
    },
}

/// A kind of people that settles, gathers, builds, learns and spreads on
/// its own. Everything a tribe can know and build is data: techs unlock
/// abilities and building plans, and plans are drawn as character grids
/// of object ids, like the object editor's patterns. Mods add new peoples
/// by adding files to `tribes/`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TribeDef {
    pub id: String,
    pub name: String,
    /// Creature used for its people (sprite, speed). A `$band` slot is
    /// painted in the tribe's own colour.
    pub creature: String,
    /// Syllables that tribe and person names are made of.
    #[serde(default)]
    pub syllables: Vec<String>,
    /// People in a new tribe (min, max).
    #[serde(default = "default_tribe_members")]
    pub members: (u32, u32),
    /// Food items and how many meals one unit is.
    pub food: Vec<(String, f32)>,
    #[serde(default = "one")]
    pub meals_per_day: f32,
    /// Lifespan in in-game days (min, max).
    #[serde(default = "default_tribe_lifespan")]
    pub lifespan_days: (f32, f32),
    /// A band leaves to found a new tribe at this many people.
    #[serde(default = "default_split_at")]
    pub split_at: u32,
    #[serde(default = "default_tribe_max")]
    pub max_members: u32,
    /// How far from its centre (tiles) a tribe works.
    #[serde(default = "default_tribe_radius")]
    pub radius: i32,
    /// Object placed at the village centre when it is founded.
    #[serde(default)]
    pub totem: Option<String>,
    #[serde(default)]
    pub start_items: Vec<(String, u32)>,
    /// Techs every new tribe of this kind knows.
    #[serde(default)]
    pub start_techs: Vec<String>,
    #[serde(default)]
    pub techs: Vec<TechDef>,
    #[serde(default)]
    pub plans: Vec<PlanDef>,
    /// Creature tags it hunts (needs a tech that unlocks "hunt").
    #[serde(default = "default_prey")]
    pub prey: Vec<String>,
    /// What it plants when farming (needs a tech that unlocks "farm").
    #[serde(default)]
    pub crop: Option<String>,
    /// Job ids used for each kind of work.
    #[serde(default)]
    pub jobs: TribeJobs,
    /// Items counted as wood and stone.
    #[serde(default = "default_wood")]
    pub wood: String,
    #[serde(default = "default_stone")]
    pub stone: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TribeJobs {
    #[serde(default = "job_harvest")]
    pub harvest: String,
    #[serde(default = "job_chop")]
    pub chop: String,
    #[serde(default = "job_quarry")]
    pub quarry: String,
    #[serde(default = "job_plant")]
    pub plant: String,
}

impl Default for TribeJobs {
    fn default() -> Self {
        Self {
            harvest: job_harvest(),
            chop: job_chop(),
            quarry: job_quarry(),
            plant: job_plant(),
        }
    }
}

fn job_harvest() -> String {
    "core:harvest".into()
}
fn job_chop() -> String {
    "core:chop".into()
}
fn job_quarry() -> String {
    "core:quarry".into()
}
fn job_plant() -> String {
    "core:plant".into()
}
fn default_wood() -> String {
    "core:wood".into()
}
fn default_stone() -> String {
    "core:stone".into()
}
fn default_prey() -> Vec<String> {
    vec!["prey".into()]
}
fn default_tribe_members() -> (u32, u32) {
    (4, 5)
}
fn default_tribe_lifespan() -> (f32, f32) {
    (60.0, 90.0)
}
fn default_split_at() -> u32 {
    10
}
fn default_tribe_max() -> u32 {
    14
}
fn default_tribe_radius() -> i32 {
    22
}

/// Something a tribe can discover. `unlocks` are abilities: `harvest`,
/// `chop`, `quarry`, `hunt`, `farm`. Plans name the tech they need.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TechDef {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Techs that must be known first.
    #[serde(default)]
    pub after: Vec<String>,
    /// Research needed. A tribe makes about 1.5 points per person per day.
    #[serde(default = "default_tech_points")]
    pub points: f32,
    #[serde(default)]
    pub unlocks: Vec<String>,
}

fn default_tech_points() -> f32 {
    30.0
}

/// A building a tribe knows how to put up. Each character in `rows` is a
/// tile; the palette lists the objects built on that tile (walls, beds,
/// roofs...). Roofs and anything else that needs support go up last.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlanDef {
    pub id: String,
    pub name: String,
    /// Tech needed to build it.
    #[serde(default)]
    pub tech: Option<String>,
    /// `home` (adds beds), `fire` (one per village), or anything else.
    #[serde(default = "default_role")]
    pub role: String,
    pub rows: Vec<String>,
    pub palette: BTreeMap<char, Vec<String>>,
}

fn default_role() -> String {
    "home".into()
}

impl Def for TribeDef {
    fn def_id(&self) -> &str {
        &self.id
    }
}
