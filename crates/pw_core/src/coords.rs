use serde::{Deserialize, Serialize};

/// Particles per tile edge.
pub const TILE: i32 = 8;
/// Tiles per chunk edge.
pub const CHUNK_TILES: i32 = 32;
/// Particles per chunk edge.
pub const CHUNK: i32 = TILE * CHUNK_TILES;
pub const CHUNK_CELLS: usize = (CHUNK * CHUNK) as usize;
pub const CHUNK_TILE_COUNT: usize = (CHUNK_TILES * CHUNK_TILES) as usize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default)]
pub struct ChunkPos {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default)]
pub struct TilePos {
    pub x: i32,
    pub y: i32,
}

impl ChunkPos {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
    /// Bottom-left particle of this chunk.
    pub fn origin(self) -> (i32, i32) {
        (self.x * CHUNK, self.y * CHUNK)
    }
}

impl TilePos {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
    /// Bottom-left particle of this tile.
    pub fn origin(self) -> (i32, i32) {
        (self.x * TILE, self.y * TILE)
    }
    /// Centre of the tile in particle space.
    pub fn center(self) -> (f32, f32) {
        (
            (self.x * TILE) as f32 + TILE as f32 / 2.0,
            (self.y * TILE) as f32 + TILE as f32 / 2.0,
        )
    }
    pub fn chunk(self) -> ChunkPos {
        ChunkPos::new(self.x.div_euclid(CHUNK_TILES), self.y.div_euclid(CHUNK_TILES))
    }
    /// Index of this tile inside its chunk's tile array.
    pub fn local_index(self) -> usize {
        let lx = self.x.rem_euclid(CHUNK_TILES);
        let ly = self.y.rem_euclid(CHUNK_TILES);
        (ly * CHUNK_TILES + lx) as usize
    }
    pub fn offset(self, dx: i32, dy: i32) -> Self {
        Self::new(self.x + dx, self.y + dy)
    }
    pub fn chebyshev(self, o: TilePos) -> i32 {
        (self.x - o.x).abs().max((self.y - o.y).abs())
    }
    pub fn manhattan(self, o: TilePos) -> i32 {
        (self.x - o.x).abs() + (self.y - o.y).abs()
    }
}

pub fn tile_of(px: i32, py: i32) -> TilePos {
    TilePos::new(px.div_euclid(TILE), py.div_euclid(TILE))
}

pub fn tile_of_f(px: f32, py: f32) -> TilePos {
    tile_of(px.floor() as i32, py.floor() as i32)
}

pub fn chunk_of(px: i32, py: i32) -> ChunkPos {
    ChunkPos::new(px.div_euclid(CHUNK), py.div_euclid(CHUNK))
}

/// Index of a particle inside its chunk's cell arrays.
pub fn cell_index(px: i32, py: i32) -> usize {
    let lx = px.rem_euclid(CHUNK);
    let ly = py.rem_euclid(CHUNK);
    (ly * CHUNK + lx) as usize
}

/// Inclusive rectangle of tiles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TileRect {
    pub min: TilePos,
    pub max: TilePos,
}

impl TileRect {
    pub fn from_corners(a: TilePos, b: TilePos) -> Self {
        Self {
            min: TilePos::new(a.x.min(b.x), a.y.min(b.y)),
            max: TilePos::new(a.x.max(b.x), a.y.max(b.y)),
        }
    }
    pub fn contains(&self, t: TilePos) -> bool {
        t.x >= self.min.x && t.x <= self.max.x && t.y >= self.min.y && t.y <= self.max.y
    }
    pub fn iter(&self) -> impl Iterator<Item = TilePos> + '_ {
        (self.min.y..=self.max.y)
            .flat_map(move |y| (self.min.x..=self.max.x).map(move |x| TilePos::new(x, y)))
    }
    pub fn width(&self) -> i32 {
        self.max.x - self.min.x + 1
    }
    pub fn height(&self) -> i32 {
        self.max.y - self.min.y + 1
    }
}
