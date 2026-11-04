//! The particle world: chunks of 256x256 cells, each cell with three layers
//! (floor, solid, overlay), plus the coarse tile grid derived from it.

use std::collections::{BTreeMap, HashMap, HashSet};

use pw_core::*;
use serde::{Deserialize, Serialize};

pub type ObjId = u32;
pub const NO_OBJ: ObjId = 0;

/// Pathing summary of an 8x8 tile, rebuilt when its particles change.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct TileInfo {
    pub walkable: bool,
    /// Movement cost multiplier (1 = normal).
    pub cost: f32,
    /// Number of solid particles in this tile.
    pub solid: u8,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Chunk {
    pub pos: ChunkPos,
    /// Material per layer per cell.
    pub mat: [Vec<MatId>; 3],
    /// Object instance owning the particle, per layer.
    pub obj: [Vec<ObjId>; 3],
    /// Per-particle shade (128 = base colour).
    pub shade: Vec<u8>,
    #[serde(skip)]
    pub tiles: Vec<TileInfo>,
    #[serde(skip)]
    pub tile_dirty: Vec<bool>,
    #[serde(skip)]
    pub any_tile_dirty: bool,
    #[serde(skip)]
    pub render_dirty: bool,
    /// Changed area (local min x, min y, max x, max y) when only part needs
    /// redrawing; `None` with `render_dirty` means redraw everything.
    #[serde(skip)]
    pub render_rect: Option<(i32, i32, i32, i32)>,
    /// Changed since generation: must be kept (and saved) instead of regenerated.
    pub modified: bool,
    /// Cells with active materials: (cell index, layer).
    #[serde(skip)]
    pub active: Vec<(u32, u8)>,
    #[serde(skip)]
    pub active_flag: Vec<u8>,
}

impl Chunk {
    pub fn new(pos: ChunkPos) -> Self {
        Self {
            pos,
            mat: std::array::from_fn(|_| vec![NONE_MAT; CHUNK_CELLS]),
            obj: std::array::from_fn(|_| vec![NO_OBJ; CHUNK_CELLS]),
            shade: vec![128; CHUNK_CELLS],
            tiles: vec![TileInfo::default(); CHUNK_TILE_COUNT],
            tile_dirty: vec![true; CHUNK_TILE_COUNT],
            any_tile_dirty: true,
            render_dirty: true,
            render_rect: None,
            modified: false,
            active: Vec::new(),
            active_flag: vec![0; CHUNK_CELLS],
        }
    }

    /// Restore the runtime-only fields after deserializing.
    pub fn after_load(&mut self, content: &Content) {
        self.tiles = vec![TileInfo::default(); CHUNK_TILE_COUNT];
        self.tile_dirty = vec![true; CHUNK_TILE_COUNT];
        self.any_tile_dirty = true;
        self.mark_render_all();
        self.active_flag = vec![0; CHUNK_CELLS];
        self.active.clear();
        self.scan_active(content);
    }

    pub fn scan_active(&mut self, content: &Content) {
        for l in Layer::ALL {
            for i in 0..CHUNK_CELLS {
                let m = self.mat[l.index()][i];
                if m != NONE_MAT && content.reaction_rt.is_active(l, m) {
                    self.mark_active(i, l);
                }
            }
        }
    }

    pub fn mark_active(&mut self, i: usize, l: Layer) {
        let bit = 1u8 << l.index();
        if self.active_flag[i] & bit == 0 {
            self.active_flag[i] |= bit;
            self.active.push((i as u32, l as u8));
        }
    }

    fn mark_cell_dirty(&mut self, i: usize) {
        let lx = (i as i32) % CHUNK;
        let ly = (i as i32) / CHUNK;
        let t = ((ly / TILE) * CHUNK_TILES + lx / TILE) as usize;
        self.tile_dirty[t] = true;
        self.any_tile_dirty = true;
        if !self.render_dirty {
            self.render_dirty = true;
            self.render_rect = Some((lx, ly, lx, ly));
        } else if let Some(r) = &mut self.render_rect {
            *r = (r.0.min(lx), r.1.min(ly), r.2.max(lx), r.3.max(ly));
        }
    }

    /// Redraw the whole chunk next frame.
    pub fn mark_render_all(&mut self) {
        self.render_dirty = true;
        self.render_rect = None;
    }

    pub fn rebuild_tiles(&mut self, content: &Content) {
        if !self.any_tile_dirty {
            return;
        }
        for t in 0..CHUNK_TILE_COUNT {
            if !self.tile_dirty[t] {
                continue;
            }
            self.tile_dirty[t] = false;
            let tx = (t as i32) % CHUNK_TILES;
            let ty = (t as i32) / CHUNK_TILES;
            let mut blocked = 0u32;
            let mut solid = 0u32;
            let mut no_floor = 0u32;
            let mut cost = 0.0f32;
            for y in 0..TILE {
                for x in 0..TILE {
                    let i = ((ty * TILE + y) * CHUNK + tx * TILE + x) as usize;
                    let s = self.mat[1][i];
                    if s != NONE_MAT {
                        solid += 1;
                        if content.mats[s as usize].blocks {
                            blocked += 1;
                        }
                    }
                    let f = &content.mats[self.mat[0][i] as usize];
                    if f.walk_cost <= 0.0 {
                        no_floor += 1;
                    } else {
                        cost += f.walk_cost;
                    }
                }
            }
            let n = (TILE * TILE) as u32;
            let walkable = blocked * 4 < n && no_floor * 2 < n;
            let avg = if n > no_floor { cost / (n - no_floor) as f32 } else { 1.0 };
            self.tiles[t] = TileInfo {
                walkable,
                cost: avg * (1.0 + blocked as f32 / n as f32 * 2.0),
                solid: solid.min(255) as u8,
            };
        }
        self.any_tile_dirty = false;
    }
}

/// A placed group of particles: a tree, a wall, a bush.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ObjectInstance {
    pub id: ObjId,
    pub def: usize,
    /// Origin particle (meaning depends on the def's anchor).
    pub origin: (i32, i32),
    pub seed: u64,
    /// Particles in the full shape.
    pub total: u32,
    /// Particles lost to work, fire, etc.
    pub removed: u32,
    /// Inclusive particle bounds.
    pub min: (i32, i32),
    pub max: (i32, i32),
    /// Set for objects placed by world generation.
    pub natural_key: Option<u64>,
    pub age: u32,
    pub built: bool,
}

impl ObjectInstance {
    pub fn origin_tile(&self) -> TilePos {
        tile_of(self.origin.0, self.origin.1)
    }
    pub fn integrity(&self) -> f32 {
        1.0 - self.removed as f32 / self.total.max(1) as f32
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct ObjectStore {
    pub next_id: ObjId,
    pub instances: BTreeMap<ObjId, ObjectInstance>,
    /// natural key -> instance id
    pub natural: HashMap<u64, ObjId>,
    /// natural objects that no longer exist; world gen skips them.
    pub destroyed_natural: HashSet<u64>,
}

impl ObjectStore {
    pub fn alloc(&mut self) -> ObjId {
        self.next_id += 1;
        self.next_id
    }
    pub fn get(&self, id: ObjId) -> Option<&ObjectInstance> {
        self.instances.get(&id)
    }
}

#[derive(Default)]
pub struct World {
    pub seed: u64,
    pub chunks: FastMap<ChunkPos, Chunk>,
    pub objects: ObjectStore,
    /// Objects that lost particles this tick and may need to break.
    pub damaged: FastSet<ObjId>,
}

impl World {
    pub fn new(seed: u64) -> Self {
        Self { seed, ..Default::default() }
    }

    pub fn chunk(&self, c: ChunkPos) -> Option<&Chunk> {
        self.chunks.get(&c)
    }

    pub fn is_loaded(&self, px: i32, py: i32) -> bool {
        self.chunks.contains_key(&chunk_of(px, py))
    }

    pub fn get(&self, px: i32, py: i32, l: Layer) -> MatId {
        self.chunks.get(&chunk_of(px, py)).map(|c| c.mat[l.index()][cell_index(px, py)]).unwrap_or(NONE_MAT)
    }

    pub fn get_obj(&self, px: i32, py: i32, l: Layer) -> ObjId {
        self.chunks.get(&chunk_of(px, py)).map(|c| c.obj[l.index()][cell_index(px, py)]).unwrap_or(NO_OBJ)
    }

    /// Set a particle. If it replaces a particle that belonged to an object,
    /// that object counts the loss (and may break).
    pub fn set(&mut self, content: &Content, px: i32, py: i32, l: Layer, m: MatId, obj: ObjId) -> bool {
        self.set_inner(content, px, py, l, m, obj, true)
    }

    /// Like `set`, but an owning object does not count the loss (harvesting, regrowth).
    pub fn set_raw(&mut self, content: &Content, px: i32, py: i32, l: Layer, m: MatId, obj: ObjId) -> bool {
        self.set_inner(content, px, py, l, m, obj, false)
    }

    #[allow(clippy::too_many_arguments)]
    fn set_inner(&mut self, content: &Content, px: i32, py: i32, l: Layer, m: MatId, obj: ObjId, count: bool) -> bool {
        let Some(c) = self.chunks.get_mut(&chunk_of(px, py)) else { return false };
        let i = cell_index(px, py);
        let li = l.index();
        let old_obj = c.obj[li][i];
        if c.mat[li][i] == m && old_obj == obj {
            return true;
        }
        c.mat[li][i] = m;
        c.obj[li][i] = obj;
        c.shade[i] = shade_for(self.seed, px, py, li);
        c.modified = true;
        c.mark_cell_dirty(i);
        if m != NONE_MAT && content.reaction_rt.is_active(l, m) {
            c.mark_active(i, l);
        }
        if count && old_obj != NO_OBJ && old_obj != obj {
            if let Some(inst) = self.objects.instances.get_mut(&old_obj) {
                inst.removed += 1;
                self.damaged.insert(old_obj);
            }
        }
        true
    }

    pub fn tile(&self, t: TilePos) -> Option<TileInfo> {
        self.chunks.get(&t.chunk()).map(|c| c.tiles[t.local_index()])
    }

    pub fn walkable(&self, t: TilePos) -> bool {
        self.tile(t).is_some_and(|i| i.walkable)
    }

    pub fn rebuild_tiles(&mut self, content: &Content) {
        for c in self.chunks.values_mut() {
            c.rebuild_tiles(content);
        }
    }

    /// Visit every particle of a tile.
    pub fn tile_cells(t: TilePos) -> impl Iterator<Item = (i32, i32)> {
        let (ox, oy) = t.origin();
        (0..TILE).flat_map(move |y| (0..TILE).map(move |x| (ox + x, oy + y)))
    }

    /// Object ids with particles inside a tile.
    pub fn objects_in_tile(&self, t: TilePos) -> Vec<ObjId> {
        let mut v = Vec::new();
        for (x, y) in Self::tile_cells(t) {
            for l in Layer::ALL {
                let o = self.get_obj(x, y, l);
                if o != NO_OBJ && !v.contains(&o) {
                    v.push(o);
                }
            }
        }
        v
    }

    pub fn insert_chunk(&mut self, chunk: Chunk) {
        self.chunks.insert(chunk.pos, chunk);
    }
}

/// Deterministic per-particle shade so colours have texture.
pub fn shade_for(seed: u64, px: i32, py: i32, layer: usize) -> u8 {
    (hash3(seed ^ 0x5eed, px as i64, py as i64, layer as i64) >> 56) as u8
}
