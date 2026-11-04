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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default)]
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
impl_def!(MaterialDef, ItemDef, ObjectDef, BiomeDef, ReactionDef, JobDef, CreatureDef);
