//! Seeded world generation. Every chunk is a pure function of
//! (seed, chunk position, content), so unmodified chunks can be dropped and
//! regenerated, and the same seed always makes the same world.

use std::collections::HashSet;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use fastnoise_lite::{FastNoiseLite, FractalType, NoiseType};
use pw_core::*;
use pw_objects::{GeneratedChunk, NaturalObject};
use pw_world::*;

/// How far (particles) an object may reach outside its origin cell.
const OBJECT_MARGIN: i32 = 32;
/// Biome noise is sampled on this grid and interpolated.
const COARSE: i32 = 4;

pub struct Generator {
    pub seed: u64,
    elev: FastNoiseLite,
    moist: FastNoiseLite,
    temp: FastNoiseLite,
    warp_x: FastNoiseLite,
    warp_y: FastNoiseLite,
    detail: FastNoiseLite,
    edge: FastNoiseLite,
    warp: f32,
}

/// How much biome borders are blended (in climate units, 0..1).
const EDGE_BLEND: f32 = 0.025;

fn noise(seed: u64, salt: u64, freq: f32, octaves: i32) -> FastNoiseLite {
    let mut n = FastNoiseLite::with_seed((splitmix64(seed ^ salt) & 0x7fff_ffff) as i32);
    n.set_noise_type(Some(NoiseType::OpenSimplex2));
    n.set_frequency(Some(freq));
    if octaves > 1 {
        n.set_fractal_type(Some(FractalType::FBm));
        n.set_fractal_octaves(Some(octaves));
    }
    n
}

fn norm(v: f32) -> f32 {
    // FBm output clusters near 0; stretch it a bit.
    ((v * 1.25 + 1.0) * 0.5).clamp(0.0, 1.0)
}

/// What the terrain is at a point, before objects.
#[derive(Clone, Copy, Debug)]
pub struct Terrain {
    pub biome: usize,
    pub floor: MatId,
    pub solid: MatId,
}

impl Generator {
    pub fn new(seed: u64, content: &Content) -> Self {
        let w = &content.worldgen;
        Self {
            seed,
            elev: noise(seed, 1, w.elevation_frequency, 5),
            moist: noise(seed, 2, w.moisture_frequency, 4),
            temp: noise(seed, 3, w.temperature_frequency, 3),
            warp_x: noise(seed, 4, w.elevation_frequency * 4.0, 2),
            warp_y: noise(seed, 5, w.elevation_frequency * 4.0, 2),
            detail: noise(seed, 6, w.detail_frequency, 3),
            edge: noise(seed, 7, 0.08, 2),
            warp: w.warp,
        }
    }

    /// (elevation, moisture, temperature) in 0..1.
    pub fn climate(&self, x: f32, y: f32) -> (f32, f32, f32) {
        let wx = x + self.warp_x.get_noise_2d(x, y) * self.warp;
        let wy = y + self.warp_y.get_noise_2d(x, y) * self.warp;
        (
            norm(self.elev.get_noise_2d(wx, wy)),
            norm(self.moist.get_noise_2d(wx, wy)),
            norm(self.temp.get_noise_2d(wx, wy)),
        )
    }

    pub fn biome_for(content: &Content, e: f32, m: f32, t: f32) -> usize {
        content.biome_rt.iter().position(|b| b.matches(e, m, t)).unwrap_or(content.biome_rt.len().saturating_sub(1))
    }

    /// Biome at a particle. Climate is nudged by small-scale noise so borders
    /// between biomes fray and mix instead of being a smooth hard line.
    fn biome_at(&self, content: &Content, x: i32, y: i32, e: f32, m: f32, t: f32) -> usize {
        let n = self.edge.get_noise_2d(x as f32, y as f32);
        let w = unit(hash2(self.seed ^ 0xED6E, x as i64, y as i64)) - 0.5;
        let j = (n * 0.75 + w * 0.5) * EDGE_BLEND;
        Self::biome_for(content, e + j, m - j, t + j * 0.5)
    }

    fn detail_at(&self, x: i32, y: i32) -> f32 {
        let n = norm(self.detail.get_noise_2d(x as f32, y as f32));
        (n * 0.85 + unit(hash2(self.seed ^ 77, x as i64, y as i64)) * 0.15).clamp(0.0, 1.0)
    }

    /// Terrain at a single particle (slow path; used for object checks).
    pub fn terrain_at(&self, content: &Content, x: i32, y: i32) -> Terrain {
        let (e, m, t) = self.climate(x as f32, y as f32);
        let bi = self.biome_at(content, x, y, e, m, t);
        let b = &content.biome_rt[bi];
        let floor = b.floor_at(self.detail_at(x, y));
        let solid = b.solid.unwrap_or(NONE_MAT);
        Terrain { biome: bi, floor, solid }
    }

    /// Generate one chunk with its natural objects.
    pub fn generate(&self, content: &Content, cp: ChunkPos) -> GeneratedChunk {
        let mut chunk = Chunk::new(cp);
        let (ox, oy) = cp.origin();
        let n = CHUNK / COARSE + 1;
        let mut clim = Vec::with_capacity((n * n) as usize);
        for gy in 0..n {
            for gx in 0..n {
                clim.push(self.climate((ox + gx * COARSE) as f32, (oy + gy * COARSE) as f32));
            }
        }
        // Veins are built per biome lazily.
        let vein_noise: Vec<Vec<FastNoiseLite>> = content
            .biome_rt
            .iter()
            .map(|b| {
                b.veins
                    .iter()
                    .enumerate()
                    .map(|(k, &(m, _, f))| noise(self.seed, 100 + k as u64 + m as u64 * 31, f, 2))
                    .collect()
            })
            .collect();
        for ly in 0..CHUNK {
            for lx in 0..CHUNK {
                let (gx, gy) = (lx / COARSE, ly / COARSE);
                let (fx, fy) = ((lx % COARSE) as f32 / COARSE as f32, (ly % COARSE) as f32 / COARSE as f32);
                let at = |x: i32, y: i32| clim[(y * n + x) as usize];
                let (a, b, c, d) = (at(gx, gy), at(gx + 1, gy), at(gx, gy + 1), at(gx + 1, gy + 1));
                let lerp = |p: f32, q: f32, r: f32, s: f32| {
                    let top = p + (q - p) * fx;
                    let bot = r + (s - r) * fx;
                    top + (bot - top) * fy
                };
                let e = lerp(a.0, b.0, c.0, d.0);
                let m = lerp(a.1, b.1, c.1, d.1);
                let t = lerp(a.2, b.2, c.2, d.2);
                let (x, y) = (ox + lx, oy + ly);
                let bi = self.biome_at(content, x, y, e, m, t);
                let biome = &content.biome_rt[bi];
                let i = (ly * CHUNK + lx) as usize;
                chunk.mat[0][i] = biome.floor_at(self.detail_at(x, y));
                if let Some(s) = biome.solid {
                    let mut solid = s;
                    for (k, &(vm, thr, _)) in biome.veins.iter().enumerate() {
                        if norm(vein_noise[bi][k].get_noise_2d(x as f32, y as f32)) > thr {
                            solid = vm;
                            break;
                        }
                    }
                    chunk.mat[1][i] = solid;
                }
                chunk.shade[i] = shade_for(self.seed, x, y, 0);
            }
        }

        let objects = self.place_natural(content, cp, &mut chunk);
        GeneratedChunk { chunk, objects }
    }

    /// Find every natural object whose shape can touch this chunk and stamp the part inside it.
    fn place_natural(&self, content: &Content, cp: ChunkPos, chunk: &mut Chunk) -> Vec<NaturalObject> {
        let (ox, oy) = cp.origin();
        let (min_x, min_y) = (ox - OBJECT_MARGIN, oy - OBJECT_MARGIN);
        let (max_x, max_y) = (ox + CHUNK + OBJECT_MARGIN, oy + CHUNK + OBJECT_MARGIN);
        let mut cands: Vec<NaturalObject> = Vec::new();
        for b in content.biome_rt.iter() {
            let bid = hash_str(content.biomes.by_idx(b.def).id.as_str());
            for (ri, rule) in b.scatter.iter().enumerate() {
                let cell = rule.spacing as i32 * TILE;
                let (cx0, cy0) = (min_x.div_euclid(cell), min_y.div_euclid(cell));
                let (cx1, cy1) = (max_x.div_euclid(cell), max_y.div_euclid(cell));
                let rule_seed = self.seed ^ bid ^ splitmix64(ri as u64 + 1);
                for cy in cy0..=cy1 {
                    for cx in cx0..=cx1 {
                        let h = hash2(rule_seed, cx as i64, cy as i64);
                        if unit(h) >= rule.chance {
                            continue;
                        }
                        let hx = hash2(h, 1, 0);
                        let tiles = rule.spacing as i64;
                        let tx = cx * rule.spacing as i32 + (hx % tiles as u64) as i32;
                        let ty = cy * rule.spacing as i32 + ((hx >> 32) % tiles as u64) as i32;
                        let (px, py) = TilePos::new(tx, ty).center();
                        let origin = match content.protos[rule.object].anchor {
                            Anchor::Center => (px as i32, py as i32),
                            Anchor::Tile => TilePos::new(tx, ty).origin(),
                        };
                        if origin.0 < min_x || origin.0 >= max_x || origin.1 < min_y || origin.1 >= max_y {
                            continue;
                        }
                        let ter = self.terrain_at(content, origin.0, origin.1);
                        if content.biome_rt[ter.biome].def != b.def
                            || ter.solid != NONE_MAT
                            || content.mat_rt(ter.floor).walk_cost <= 0.0
                        {
                            continue;
                        }
                        cands.push(NaturalObject { key: h, def: rule.object, origin, seed: hash2(h, 7, 7) });
                    }
                }
            }
        }
        // Same order in every chunk, so overlapping objects resolve the same way everywhere.
        cands.sort_by_key(|c| c.key);
        let mut used = Vec::new();
        for c in cands {
            let cells = content.protos[c.def].cells(c.seed);
            let local_id = used.len() as ObjId + 1;
            let mut touched = false;
            for cell in &cells {
                let (x, y) = (c.origin.0 + cell.dx, c.origin.1 + cell.dy);
                if x < ox || y < oy || x >= ox + CHUNK || y >= oy + CHUNK {
                    continue;
                }
                let i = cell_index(x, y);
                let li = cell.layer.index();
                if cell.layer != Layer::Floor && chunk.mat[li][i] != NONE_MAT {
                    continue;
                }
                chunk.mat[li][i] = cell.mat;
                chunk.obj[li][i] = local_id;
                touched = true;
            }
            if touched {
                used.push(c);
            }
        }
        used
    }
}

/// Background chunk generation on worker threads.
pub struct GenPool {
    tx: Sender<(ChunkPos, u64, Arc<Content>)>,
    rx: Mutex<Receiver<GeneratedChunk>>,
    pub pending: HashSet<ChunkPos>,
}

impl GenPool {
    pub fn new(threads: usize) -> Self {
        let (tx, job_rx) = channel::<(ChunkPos, u64, Arc<Content>)>();
        let (done_tx, rx) = channel::<GeneratedChunk>();
        let job_rx = Arc::new(Mutex::new(job_rx));
        for _ in 0..threads.max(1) {
            let job_rx = job_rx.clone();
            let done_tx = done_tx.clone();
            std::thread::spawn(move || {
                loop {
                    let job = { job_rx.lock().unwrap().recv() };
                    let Ok((cp, seed, content)) = job else { break };
                    let g = Generator::new(seed, &content).generate(&content, cp);
                    if done_tx.send(g).is_err() {
                        break;
                    }
                }
            });
        }
        Self { tx, rx: Mutex::new(rx), pending: HashSet::new() }
    }

    pub fn request(&mut self, cp: ChunkPos, seed: u64, content: Arc<Content>) {
        if self.pending.insert(cp) {
            let _ = self.tx.send((cp, seed, content));
        }
    }

    pub fn finished(&mut self) -> Vec<GeneratedChunk> {
        let out: Vec<GeneratedChunk> = self.rx.lock().unwrap().try_iter().collect();
        for g in &out {
            self.pending.remove(&g.chunk.pos);
        }
        out
    }
}
