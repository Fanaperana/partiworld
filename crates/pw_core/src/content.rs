//! `Content` = every registry plus the compiled, numeric-id runtime tables
//! the simulation uses. Build it by inserting defs, then call `link()`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::defs::*;
use crate::registry::Registry;
use crate::rng::Rng;

pub type MatId = u16;
pub const NONE_MAT: MatId = 0;
pub const NONE_ID: &str = "none";

pub fn parse_color(s: &str) -> Option<[u8; 4]> {
    let h = s.trim().trim_start_matches('#');
    let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
    match h.len() {
        6 => Some([byte(0)?, byte(2)?, byte(4)?, 255]),
        8 => Some([byte(0)?, byte(2)?, byte(4)?, byte(6)?]),
        _ => None,
    }
}

#[derive(Clone, Debug)]
pub struct MaterialRt {
    pub color: [u8; 4],
    pub variance: f32,
    pub blocks: bool,
    pub walk_cost: f32,
    pub hardness: f32,
    /// (item index, particles per item)
    pub yields: Option<(usize, u32)>,
    pub tags: Vec<String>,
    /// Holds roofs up (tag `support`).
    pub support: bool,
    /// Tree leaves (tag `leaves`), and whether they stay in winter (`evergreen`).
    pub leaves: bool,
    pub evergreen: bool,
    /// Food growing on plants, like berries (tag `food`).
    pub fruit: bool,
    /// Tags `roof` and `water`, used for snow and ice.
    pub roof: bool,
    pub water: bool,
    /// C this material adds near it (`MaterialDef::heat`), and whether it burns to stand on.
    pub heat: f32,
    pub hot: bool,
}

impl MaterialRt {
    pub fn has_tag(&self, t: &str) -> bool {
        self.tags.iter().any(|x| x == t)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StampCell {
    pub dx: i32,
    pub dy: i32,
    pub layer: Layer,
    pub mat: MatId,
}

#[derive(Clone, Debug)]
enum PrimRt {
    Circle { layer: Layer, mat: MatId, r: (f32, f32), off: (f32, f32), density: f32 },
    Blob { layer: Layer, mat: MatId, r: (f32, f32), off: (f32, f32), rough: f32, density: f32 },
    Rect { layer: Layer, mat: MatId, size: (i32, i32), off: (f32, f32), density: f32 },
    Scatter { layer: Layer, mat: MatId, r: f32, count: (u32, u32), off: (f32, f32) },
}

#[derive(Clone, Debug)]
pub struct BuildRt {
    pub cost: Vec<(usize, u32)>,
    pub work: u32,
    pub category: String,
}

#[derive(Clone, Debug)]
pub struct RegrowRt {
    pub mats: Vec<MatId>,
    pub interval: u32,
    pub chance: f32,
}

#[derive(Clone, Debug)]
pub struct ObjectProto {
    fixed: Option<Vec<StampCell>>,
    prims: Vec<PrimRt>,
    pub tags: Vec<String>,
    pub anchor: Anchor,
    pub break_fraction: f32,
    pub drops: Vec<(usize, u32, u32)>,
    pub build: Option<BuildRt>,
    pub growth: Option<(usize, u32)>,
    pub regrow: Option<RegrowRt>,
    pub hooks: BTreeMap<String, String>,
    /// True if the shape puts anything in the solid layer that blocks.
    pub blocks: bool,
    /// Roofs: max distance (tiles) to a support; 0 = none needed.
    pub needs_support: u32,
    /// (object index, chance, radius tiles, interval ticks, max nearby).
    pub spread: Option<(usize, f32, u32, u32, u32)>,
    /// True if the shape has solid particles tagged `support` (walls).
    pub gives_support: bool,
}

impl ObjectProto {
    pub fn has_tag(&self, t: &str) -> bool {
        self.tags.iter().any(|x| x == t)
    }

    /// The particles this object is made of, for a given instance seed.
    /// Painted patterns ignore the seed.
    pub fn cells(&self, seed: u64) -> Vec<StampCell> {
        if let Some(f) = &self.fixed {
            return f.clone();
        }
        let mut rng = Rng::new(seed);
        // Later primitives overwrite earlier ones in the same layer.
        let mut map: BTreeMap<(u8, i32, i32), MatId> = BTreeMap::new();
        for p in &self.prims {
            match *p {
                PrimRt::Circle { layer, mat, r, off, density } => {
                    let rad = rng.range_f32(r.0, r.1);
                    disc(rad, off, |dx, dy, _| {
                        if rng.f32() < density {
                            map.insert((layer as u8, dx, dy), mat);
                        }
                    });
                }
                PrimRt::Blob { layer, mat, r, off, rough, density } => {
                    let rad = rng.range_f32(r.0, r.1);
                    let phases: Vec<f32> = (0..4).map(|_| rng.f32() * std::f32::consts::TAU).collect();
                    let amps: Vec<f32> = (0..4).map(|_| rng.f32()).collect();
                    let max_r = rad * (1.0 + rough);
                    disc(max_r, off, |dx, dy, d| {
                        let a = (dy as f32 - off.1).atan2(dx as f32 - off.0);
                        let mut w = 0.0;
                        for k in 0..4 {
                            w += amps[k] * ((k as f32 + 2.0) * a + phases[k]).sin() / (k as f32 + 1.0);
                        }
                        let edge = rad * (1.0 + rough * 0.5 * w);
                        if d <= edge && rng.f32() < density {
                            map.insert((layer as u8, dx, dy), mat);
                        }
                    });
                }
                PrimRt::Rect { layer, mat, size, off, density } => {
                    let x0 = (off.0 - size.0 as f32 / 2.0).round() as i32;
                    let y0 = (off.1 - size.1 as f32 / 2.0).round() as i32;
                    for y in 0..size.1 {
                        for x in 0..size.0 {
                            if rng.f32() < density {
                                map.insert((layer as u8, x0 + x, y0 + y), mat);
                            }
                        }
                    }
                }
                PrimRt::Scatter { layer, mat, r, count, off } => {
                    let n = rng.range_i32(count.0 as i32, count.1 as i32);
                    for _ in 0..n {
                        let a = rng.f32() * std::f32::consts::TAU;
                        let d = rng.f32().sqrt() * r;
                        let x = (off.0 + a.cos() * d).round() as i32;
                        let y = (off.1 + a.sin() * d).round() as i32;
                        map.insert((layer as u8, x, y), mat);
                    }
                }
            }
        }
        map.into_iter()
            .map(|((l, dx, dy), mat)| StampCell { dx, dy, layer: Layer::from_index(l as usize), mat })
            .collect()
    }
}

/// Visit every integer offset within radius `r` of `off`.
fn disc(r: f32, off: (f32, f32), mut f: impl FnMut(i32, i32, f32)) {
    let ri = r.ceil() as i32 + 1;
    let (ox, oy) = (off.0.round() as i32, off.1.round() as i32);
    for y in -ri..=ri {
        for x in -ri..=ri {
            let (px, py) = (ox + x, oy + y);
            let d = ((px as f32 - off.0).powi(2) + (py as f32 - off.1).powi(2)).sqrt();
            if d <= r {
                f(px, py, d);
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct ScatterRt {
    pub object: usize,
    pub spacing: u32,
    pub chance: f32,
}

#[derive(Clone, Debug)]
pub struct BiomeRt {
    pub def: usize,
    pub priority: i32,
    pub elevation: (f32, f32),
    pub moisture: (f32, f32),
    pub temperature: (f32, f32),
    /// (material, cumulative weight 0..1)
    pub floors: Vec<(MatId, f32)>,
    pub solid: Option<MatId>,
    pub veins: Vec<(MatId, f32, f32)>,
    pub scatter: Vec<ScatterRt>,
}

impl BiomeRt {
    pub fn matches(&self, e: f32, m: f32, t: f32) -> bool {
        let inr = |v: f32, r: (f32, f32)| v >= r.0 && v <= r.1;
        inr(e, self.elevation) && inr(m, self.moisture) && inr(t, self.temperature)
    }
    pub fn floor_at(&self, detail: f32) -> MatId {
        for &(m, c) in &self.floors {
            if detail <= c {
                return m;
            }
        }
        self.floors.last().map(|f| f.0).unwrap_or(NONE_MAT)
    }
}

#[derive(Clone, Debug)]
pub struct EffectRt {
    pub target: Target,
    pub layer: Option<Layer>,
    pub mat: MatId,
}

#[derive(Clone, Debug)]
pub struct ReactionRt {
    pub layer: Layer,
    pub neighbor: Option<(Layer, Vec<bool>, bool)>,
    pub chance: f32,
    pub effects: Vec<EffectRt>,
    pub wet: f32,
    pub windy: bool,
    pub swap: bool,
    pub temp: (f32, f32),
}

/// Reactions grouped by (mode, layer, input material) for fast lookup.
#[derive(Clone, Debug, Default)]
pub struct Reactions {
    pub list: Vec<ReactionRt>,
    /// [layer][material] -> reaction indices
    pub active: [Vec<Vec<usize>>; 3],
    pub random: [Vec<Vec<usize>>; 3],
}

impl Reactions {
    pub fn is_active(&self, layer: Layer, mat: MatId) -> bool {
        self.active[layer.index()].get(mat as usize).is_some_and(|v| !v.is_empty())
    }
}

#[derive(Clone, Debug)]
pub struct SpriteRt {
    pub w: u32,
    pub h: u32,
    /// Row-major, top row first. Slot pixels hold the slot index in `slots` until filled.
    pub pixels: Vec<[u8; 4]>,
    pub slots: Vec<(usize, String)>,
    /// Extra frames (drawn facings, then poses); frame 0 is this sprite
    /// itself, frame i is `views[i - 1]`.
    pub views: Vec<SpriteRt>,
    /// Action pose name -> frame.
    pub poses: BTreeMap<String, u8>,
    /// For each of the 8 directions (see `FACINGS`): which frame to show
    /// and whether to mirror it.
    pub facing: [(u8, bool); 8],
    /// Whether the renderer adds an outline.
    pub outline: bool,
}

/// Directions in facing order: index 0 faces the viewer, then turning
/// through east. `facing_of` maps a movement onto these.
pub const FACINGS: [&str; 8] = ["s", "se", "e", "ne", "n", "nw", "w", "sw"];

/// Facing index (into `FACINGS`) for a movement, with +y pointing north.
pub fn facing_of(dx: f32, dy: f32) -> usize {
    let a = dx.atan2(-dy) / std::f32::consts::FRAC_PI_4;
    (a.round() as i32).rem_euclid(8) as usize
}

impl SpriteRt {
    pub fn compile(def: &SpriteDef) -> Result<SpriteRt, String> {
        let mut base = Self::frame(&def.palette, &def.rows)?;
        base.outline = def.outline;
        // Frame per drawn facing; index 0 is the front view.
        let mut drawn: [Option<u8>; 8] = [None; 8];
        drawn[0] = Some(0);
        for (name, rows) in &def.views {
            let d = FACINGS.iter().position(|f| f == name).ok_or(format!("unknown sprite view '{name}'"))?;
            let mut f = Self::frame(&def.palette, rows)?;
            f.outline = def.outline;
            base.views.push(f);
            drawn[d] = Some(base.views.len() as u8);
        }
        for d in 0..8 {
            base.facing[d] = Self::resolve(&drawn, d);
        }
        for (name, rows) in &def.poses {
            let mut f = Self::frame(&def.palette, rows)?;
            f.outline = def.outline;
            base.views.push(f);
            base.poses.insert(name.clone(), base.views.len() as u8);
        }
        Ok(base)
    }

    /// Frame `i` (0 = the front view).
    pub fn frame_rt(&self, i: u8) -> &SpriteRt {
        if i == 0 { self } else { self.views.get(i as usize - 1).unwrap_or(self) }
    }

    /// Pick a frame for facing `d`: its own drawing, else its mirror image,
    /// else the nearest drawn facing (side views first, as they show movement best).
    fn resolve(drawn: &[Option<u8>; 8], d: usize) -> (u8, bool) {
        let mirror = |d: usize| (8 - d) % 8;
        let pick = |d: usize| drawn[d].map(|f| (f, false)).or(drawn[mirror(d)].map(|f| (f, true)));
        for k in 0..=4i32 {
            let mut near = [(d as i32 + k).rem_euclid(8) as usize, (d as i32 - k).rem_euclid(8) as usize];
            near.sort_by_key(|&n| !(n == 2 || n == 6));
            if let Some(f) = near.iter().find_map(|&n| pick(n)) {
                return f;
            }
        }
        (0, false)
    }

    fn frame(palette: &BTreeMap<char, String>, rows: &[String]) -> Result<SpriteRt, String> {
        let h = rows.len() as u32;
        let w = rows.iter().map(|r| r.chars().count()).max().unwrap_or(0) as u32;
        let mut pixels = vec![[0u8; 4]; (w * h) as usize];
        let mut slots = Vec::new();
        for (y, row) in rows.iter().enumerate() {
            for (x, ch) in row.chars().enumerate() {
                let i = y * w as usize + x;
                if ch == ' ' || ch == '.' {
                    continue;
                }
                match palette.get(&ch) {
                    Some(c) if c.starts_with('$') => slots.push((i, c[1..].to_string())),
                    Some(c) => pixels[i] = parse_color(c).ok_or(format!("bad colour '{c}'"))?,
                    None => return Err(format!("sprite char '{ch}' not in palette")),
                }
            }
        }
        Ok(SpriteRt { w, h, pixels, slots, views: Vec::new(), poses: BTreeMap::new(), facing: [(0, false); 8], outline: true })
    }

    /// A little pile of particles in one colour, for items without a sprite.
    pub fn pile(color: [u8; 4]) -> SpriteRt {
        const ROWS: [&str; 5] = [" ## ", "#%##", "##%#", "#%%#", " ## "];
        let mut pixels = Vec::new();
        for r in ROWS {
            for c in r.chars() {
                let f = match c {
                    '#' => 1.0,
                    '%' => 0.75,
                    _ => -1.0,
                };
                if f < 0.0 {
                    pixels.push([0, 0, 0, 0]);
                } else {
                    pixels.push([
                        (color[0] as f32 * f) as u8,
                        (color[1] as f32 * f) as u8,
                        (color[2] as f32 * f) as u8,
                        255,
                    ]);
                }
            }
        }
        SpriteRt { w: 4, h: 5, pixels, slots: Vec::new(), views: Vec::new(), poses: BTreeMap::new(), facing: [(0, false); 8], outline: true }
    }
}

#[derive(Clone, Debug)]
pub struct ItemRt {
    pub color: [u8; 4],
    pub stack: u32,
    pub sprite: SpriteRt,
}

#[derive(Clone, Debug)]
pub struct CreatureRt {
    pub sprite: SpriteRt,
}

#[derive(Clone, Debug)]
pub struct Content {
    pub mods: Vec<ModInfo>,
    pub materials: Registry<MaterialDef>,
    pub items: Registry<ItemDef>,
    pub objects: Registry<ObjectDef>,
    pub biomes: Registry<BiomeDef>,
    pub reactions: Registry<ReactionDef>,
    pub jobs: Registry<JobDef>,
    pub creatures: Registry<CreatureDef>,
    pub powers: Registry<PowerDef>,
    pub tribes: Registry<TribeDef>,
    pub worldgen: WorldGenDef,
    pub climate: ClimateDef,

    // Compiled by `link()`.
    pub mats: Vec<MaterialRt>,
    pub item_rt: Vec<ItemRt>,
    pub protos: Vec<ObjectProto>,
    pub biome_rt: Vec<BiomeRt>,
    pub reaction_rt: Reactions,
    pub creature_rt: Vec<CreatureRt>,
}

impl Default for Content {
    fn default() -> Self {
        Self::new()
    }
}

impl Content {
    pub fn new() -> Self {
        let mut materials = Registry::new();
        materials.insert(
            MaterialDef {
                id: NONE_ID.into(),
                name: "Nothing".into(),
                color: "#00000000".into(),
                variance: 0.0,
                tags: vec![],
                blocks: false,
                walk_cost: 1.0,
                hardness: 1.0,
                yields: None,
                heat: 0.0,
            },
            "engine",
        );
        Self {
            mods: Vec::new(),
            materials,
            items: Registry::new(),
            objects: Registry::new(),
            biomes: Registry::new(),
            reactions: Registry::new(),
            jobs: Registry::new(),
            creatures: Registry::new(),
            powers: Registry::new(),
            tribes: Registry::new(),
            worldgen: WorldGenDef::default(),
            climate: ClimateDef::default(),
            mats: Vec::new(),
            item_rt: Vec::new(),
            protos: Vec::new(),
            biome_rt: Vec::new(),
            reaction_rt: Reactions::default(),
            creature_rt: Vec::new(),
        }
    }

    pub fn mat(&self, id: &str) -> Option<MatId> {
        if id == NONE_ID || id.is_empty() {
            return Some(NONE_MAT);
        }
        self.materials.idx(id).map(|i| i as MatId)
    }
    /// Which materials a pattern matches: a material id, `tag:<tag>`, `none` or `any`.
    /// Returns one flag per material index (None if the id is unknown).
    pub fn mat_mask(&self, spec: &str) -> Option<Vec<bool>> {
        let n = self.mats.len();
        if spec == "any" {
            return Some(vec![true; n]);
        }
        if let Some(tag) = spec.strip_prefix("tag:") {
            return Some(self.mats.iter().map(|r| r.has_tag(tag)).collect());
        }
        let id = self.mat(spec)? as usize;
        Some((0..n).map(|i| i == id).collect())
    }

    pub fn mat_rt(&self, m: MatId) -> &MaterialRt {
        &self.mats[m as usize]
    }
    pub fn mat_name(&self, m: MatId) -> &str {
        self.materials.try_idx(m as usize).map(|d| d.name.as_str()).unwrap_or("?")
    }
    pub fn mat_id_str(&self, m: MatId) -> &str {
        self.materials.try_idx(m as usize).map(|d| d.id.as_str()).unwrap_or(NONE_ID)
    }

    /// Resolve all string references and build the runtime tables.
    /// Returns every problem found instead of stopping at the first.
    pub fn link(&mut self) -> Result<(), Vec<String>> {
        let mut errs: Vec<String> = Vec::new();

        // Items first: materials reference them.
        self.item_rt = self
            .items
            .iter()
            .map(|(_, d)| {
                let color = parse_color(&d.color).unwrap_or_else(|| {
                    errs.push(format!("item {}: bad colour '{}'", d.id, d.color));
                    [255, 0, 255, 255]
                });
                let sprite = match &d.sprite {
                    Some(s) => SpriteRt::compile(s).unwrap_or_else(|e| {
                        errs.push(format!("item {}: {e}", d.id));
                        SpriteRt::pile(color)
                    }),
                    None => SpriteRt::pile(color),
                };
                ItemRt { color, stack: d.stack.max(1), sprite }
            })
            .collect();

        let items = &self.items;
        self.mats = self
            .materials
            .iter()
            .map(|(_, d)| {
                let color = parse_color(&d.color).unwrap_or_else(|| {
                    errs.push(format!("material {}: bad colour '{}'", d.id, d.color));
                    [255, 0, 255, 255]
                });
                let yields = d.yields.as_ref().and_then(|y| match items.idx(&y.item) {
                    Some(i) => Some((i, y.per.max(1))),
                    None => {
                        errs.push(format!("material {}: unknown yield item '{}'", d.id, y.item));
                        None
                    }
                });
                MaterialRt {
                    color,
                    variance: d.variance.clamp(0.0, 1.0),
                    blocks: d.blocks,
                    walk_cost: d.walk_cost.max(0.0),
                    hardness: d.hardness.max(0.05),
                    yields,
                    tags: d.tags.clone(),
                    support: d.tags.iter().any(|t| t == "support"),
                    leaves: d.tags.iter().any(|t| t == "leaves"),
                    evergreen: d.tags.iter().any(|t| t == "evergreen"),
                    fruit: d.tags.iter().any(|t| t == "food"),
                    roof: d.tags.iter().any(|t| t == "roof"),
                    heat: d.heat,
                    hot: d.tags.iter().any(|t| t == "hot"),
                    water: d.tags.iter().any(|t| t == "water"),
                }
            })
            .collect();

        let mat_ref = |id: &str, ctx: &str, errs: &mut Vec<String>| -> MatId {
            if id == NONE_ID {
                return NONE_MAT;
            }
            match self.materials.idx(id) {
                Some(i) => i as MatId,
                None => {
                    errs.push(format!("{ctx}: unknown material '{id}'"));
                    NONE_MAT
                }
            }
        };
        let item_ref = |id: &str, ctx: &str, errs: &mut Vec<String>| -> Option<usize> {
            let r = self.items.idx(id);
            if r.is_none() {
                errs.push(format!("{ctx}: unknown item '{id}'"));
            }
            r
        };

        // Objects.
        let mut protos = Vec::new();
        for (_, d) in self.objects.iter() {
            let ctx = format!("object {}", d.id);
            let mut fixed = None;
            let mut prims = Vec::new();
            match &d.shape {
                ShapeDef::Pattern { palette, layers } => {
                    let mut cells = Vec::new();
                    for pl in layers {
                        let h = pl.rows.len() as i32;
                        let w = pl.rows.iter().map(|r| r.chars().count()).max().unwrap_or(0) as i32;
                        for (row, line) in pl.rows.iter().enumerate() {
                            for (col, ch) in line.chars().enumerate() {
                                if ch == ' ' || ch == '.' {
                                    continue;
                                }
                                let Some(mid) = palette.get(&ch) else {
                                    errs.push(format!("{ctx}: char '{ch}' not in palette"));
                                    continue;
                                };
                                let mat = mat_ref(mid, &ctx, &mut errs);
                                let (mut dx, mut dy) = (col as i32, h - 1 - row as i32);
                                if d.anchor == Anchor::Center {
                                    dx -= w / 2;
                                    dy -= h / 2;
                                }
                                cells.push(StampCell { dx, dy, layer: pl.layer, mat });
                            }
                        }
                    }
                    fixed = Some(cells);
                }
                ShapeDef::Procedural(list) => {
                    for p in list {
                        prims.push(match p {
                            Primitive::Circle { layer, material, radius, offset, density } => PrimRt::Circle {
                                layer: *layer,
                                mat: mat_ref(material, &ctx, &mut errs),
                                r: *radius,
                                off: *offset,
                                density: *density,
                            },
                            Primitive::Blob { layer, material, radius, offset, roughness, density } => PrimRt::Blob {
                                layer: *layer,
                                mat: mat_ref(material, &ctx, &mut errs),
                                r: *radius,
                                off: *offset,
                                rough: *roughness,
                                density: *density,
                            },
                            Primitive::Rect { layer, material, size, offset, density } => PrimRt::Rect {
                                layer: *layer,
                                mat: mat_ref(material, &ctx, &mut errs),
                                size: *size,
                                off: *offset,
                                density: *density,
                            },
                            Primitive::Scatter { layer, material, radius, count, offset } => PrimRt::Scatter {
                                layer: *layer,
                                mat: mat_ref(material, &ctx, &mut errs),
                                r: *radius,
                                count: *count,
                                off: *offset,
                            },
                        });
                    }
                }
            }
            let drops = d
                .drops
                .iter()
                .filter_map(|dr| item_ref(&dr.item, &ctx, &mut errs).map(|i| (i, dr.min, dr.max.max(dr.min))))
                .collect();
            let build = d.build.as_ref().map(|b| BuildRt {
                cost: b
                    .cost
                    .iter()
                    .filter_map(|(id, n)| item_ref(id, &ctx, &mut errs).map(|i| (i, *n)))
                    .collect(),
                work: b.work.max(1),
                category: if b.category.is_empty() { "misc".into() } else { b.category.clone() },
            });
            let regrow = d.regrow.as_ref().map(|r| RegrowRt {
                mats: r.materials.iter().map(|m| mat_ref(m, &ctx, &mut errs)).collect(),
                interval: r.interval.max(1),
                chance: r.chance,
            });
            protos.push(ObjectProto {
                fixed,
                prims,
                tags: d.tags.clone(),
                anchor: d.anchor,
                break_fraction: d.break_fraction.clamp(0.01, 1.0),
                drops,
                build,
                growth: None,
                regrow,
                hooks: d.hooks.clone(),
                blocks: false,
                needs_support: d.needs_support,
                gives_support: false,
                spread: None,
            });
        }
        // Growth and spread targets need every object registered.
        for (i, d) in self.objects.iter() {
            if let Some(s) = &d.spread {
                match self.objects.idx(&s.object) {
                    Some(n) => protos[i].spread = Some((n, s.chance, s.radius.max(1), s.interval.max(30), s.max_nearby)),
                    None => errs.push(format!("object {}: unknown spread object '{}'", d.id, s.object)),
                }
            }
            if let Some(g) = &d.growth {
                match self.objects.idx(&g.next) {
                    Some(n) => protos[i].growth = Some((n, g.ticks.max(1))),
                    None => errs.push(format!("object {}: unknown growth target '{}'", d.id, g.next)),
                }
            }
        }
        for p in &mut protos {
            let cells = p.cells(0);
            p.blocks = cells.iter().any(|c| c.layer == Layer::Solid && self.mats.get(c.mat as usize).is_some_and(|m| m.blocks));
            p.gives_support =
                cells.iter().any(|c| c.layer == Layer::Solid && self.mats.get(c.mat as usize).is_some_and(|m| m.blocks && m.support));
        }
        self.protos = protos;

        // Biomes.
        let mut biomes = Vec::new();
        for (i, d) in self.biomes.iter() {
            let ctx = format!("biome {}", d.id);
            let total: f32 = d.floor.iter().map(|f| f.1.max(0.0)).sum::<f32>().max(1e-6);
            let mut acc = 0.0;
            let floors = d
                .floor
                .iter()
                .map(|(m, w)| {
                    acc += w.max(0.0) / total;
                    (mat_ref(m, &ctx, &mut errs), acc)
                })
                .collect();
            let scatter = d
                .scatter
                .iter()
                .filter_map(|s| match self.objects.idx(&s.object) {
                    Some(o) => Some(ScatterRt { object: o, spacing: s.spacing.max(1), chance: s.chance }),
                    None => {
                        errs.push(format!("{ctx}: unknown object '{}'", s.object));
                        None
                    }
                })
                .collect();
            biomes.push(BiomeRt {
                def: i,
                priority: d.priority,
                elevation: d.elevation,
                moisture: d.moisture,
                temperature: d.temperature,
                floors,
                solid: d.solid.as_ref().map(|s| mat_ref(s, &ctx, &mut errs)),
                veins: d.veins.iter().map(|v| (mat_ref(&v.material, &ctx, &mut errs), v.threshold, v.frequency)).collect(),
                scatter,
            });
        }
        biomes.sort_by(|a, b| b.priority.cmp(&a.priority).then(a.def.cmp(&b.def)));
        if biomes.is_empty() {
            errs.push("no biomes defined".into());
        }
        self.biome_rt = biomes;

        // Reactions.
        let nmat = self.materials.len();
        let mask = |m: &str, ctx: &str, errs: &mut Vec<String>| -> Vec<bool> {
            if m == "any" {
                return vec![true; nmat];
            }
            if let Some(tag) = m.strip_prefix("tag:") {
                return self.mats.iter().map(|r| r.has_tag(tag)).collect();
            }
            let id = mat_ref(m, ctx, errs);
            (0..nmat).map(|i| i == id as usize).collect()
        };
        let mut rx = Reactions {
            list: Vec::new(),
            active: std::array::from_fn(|_| vec![Vec::new(); nmat]),
            random: std::array::from_fn(|_| vec![Vec::new(); nmat]),
        };
        for (_, d) in self.reactions.iter() {
            let ctx = format!("reaction {}", d.id);
            let input = mask(&d.input, &ctx, &mut errs);
            let neighbor = d.neighbor.as_ref().map(|n| (n.layer, mask(&n.matches, &ctx, &mut errs), n.same_cell));
            let effects = d
                .effects
                .iter()
                .map(|e| EffectRt { target: e.target, layer: e.layer, mat: mat_ref(&e.set, &ctx, &mut errs) })
                .collect();
            let idx = rx.list.len();
            rx.list.push(ReactionRt {
                layer: d.layer,
                neighbor,
                chance: d.chance,
                effects,
                wet: d.wet.max(0.0),
                windy: d.windy,
                swap: d.swap,
                temp: (d.min_temp.unwrap_or(f32::MIN), d.max_temp.unwrap_or(f32::MAX)),
            });
            let table = match d.mode {
                ReactionMode::Active => &mut rx.active,
                ReactionMode::Random => &mut rx.random,
            };
            for (m, hit) in input.iter().enumerate() {
                if *hit && m != NONE_MAT as usize {
                    table[d.layer.index()][m].push(idx);
                }
            }
        }
        self.reaction_rt = rx;

        // Creatures.
        self.creature_rt = self
            .creatures
            .iter()
            .map(|(_, d)| CreatureRt {
                sprite: SpriteRt::compile(&d.sprite).unwrap_or_else(|e| {
                    errs.push(format!("creature {}: {e}", d.id));
                    SpriteRt::pile([255, 0, 255, 255])
                }),
            })
            .collect();

        if errs.is_empty() { Ok(()) } else { Err(errs) }
    }

    /// Jobs sorted by priority (highest first).
    pub fn jobs_by_priority(&self) -> Vec<usize> {
        let mut v: Vec<usize> = (0..self.jobs.len()).collect();
        v.sort_by_key(|&i| -self.jobs.by_idx(i).priority);
        v
    }
}

#[cfg(test)]
mod facing_tests {
    use super::*;

    fn sprite(views: &[&str]) -> SpriteRt {
        let mut def = SpriteDef { palette: [('x', "#ffffff".to_string())].into(), rows: vec!["x".into()], ..Default::default() };
        def.outline = true;
        for v in views {
            def.views.insert(v.to_string(), vec!["x".into()]);
        }
        SpriteRt::compile(&def).unwrap()
    }

    #[test]
    fn poses_come_after_views() {
        let mut def = SpriteDef { palette: [('x', "#ffffff".to_string())].into(), rows: vec!["x".into()], outline: true, ..Default::default() };
        def.views.insert("e".into(), vec!["x".into()]);
        def.poses.insert("sleep".into(), vec!["xxx".into()]);
        let s = SpriteRt::compile(&def).unwrap();
        assert_eq!(s.poses["sleep"], 2);
        assert_eq!(s.frame_rt(2).w, 3);
        assert_eq!(s.facing[2], (1, false));
    }

    #[test]
    fn movement_maps_to_facings() {
        assert_eq!(FACINGS[facing_of(0.0, -1.0)], "s");
        assert_eq!(FACINGS[facing_of(1.0, 0.0)], "e");
        assert_eq!(FACINGS[facing_of(0.0, 1.0)], "n");
        assert_eq!(FACINGS[facing_of(-1.0, 0.0)], "w");
        assert_eq!(FACINGS[facing_of(1.0, 1.0)], "ne");
        assert_eq!(FACINGS[facing_of(-1.0, -1.0)], "sw");
    }

    #[test]
    fn missing_views_mirror_or_borrow() {
        // Front only: every facing shows it.
        assert!(sprite(&[]).facing.iter().all(|&(f, _)| f == 0));
        // East drawn: west mirrors it, and diagonals borrow the side view.
        let s = sprite(&["e", "n"]);
        let e = s.facing[2].0;
        assert_eq!(s.facing[6], (e, true));
        assert_eq!(s.facing[1], (e, false));
        assert_eq!(s.facing[5], (e, true));
        assert_eq!(s.facing[4].1, false);
        // All eight drawn: nothing is mirrored.
        let all = sprite(&["se", "e", "ne", "n", "nw", "w", "sw"]);
        assert!(all.facing.iter().enumerate().all(|(d, &(f, flip))| !flip && (d == 0) == (f == 0)));
    }
}
