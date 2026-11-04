//! Particle reactions, driven entirely by `ReactionDef` data.
//! Active reactions run every tick on cells that hold an active material
//! (fire). Random reactions run on a few sampled cells per chunk per tick
//! (grass regrowth, ash turning to dirt).

use pw_core::*;
use pw_world::*;

const DIRS: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

/// Weather as the particle reactions see it.
#[derive(Clone, Copy, Debug, Default)]
pub struct SimEnv {
    /// 0 = dry, 1 = soaked.
    pub wet: f32,
    /// Direction the wind blows toward (unit vector) times its strength 0..1.
    pub wind: (f32, f32),
    /// Outdoor temperature, C.
    pub temp: f32,
}

pub struct SimSettings {
    /// Random cell samples per loaded chunk per tick.
    pub random_samples: usize,
    /// Cap on active cells processed per tick (keeps huge fires from stalling the game).
    pub max_active: usize,
}

impl Default for SimSettings {
    fn default() -> Self {
        Self { random_samples: 64, max_active: 60_000 }
    }
}

/// Pick a neighbour direction. Windy reactions favour downwind, and strong
/// wind sometimes carries them a few cells (embers).
fn pick_dir(rng: &mut Rng, r: &ReactionRt, same_cell: bool, env: &SimEnv) -> (i32, i32) {
    let n = if same_cell { 5 } else { 4 };
    let strength = (env.wind.0 * env.wind.0 + env.wind.1 * env.wind.1).sqrt();
    if !r.windy || strength < 0.05 {
        return DIRS.get(rng.below(n)).copied().unwrap_or((0, 0));
    }
    let (wx, wy) = (env.wind.0 / strength, env.wind.1 / strength);
    let mut weights = [0.0f32; 5];
    for (i, &(dx, dy)) in DIRS.iter().enumerate() {
        weights[i] = (1.0 + 2.5 * strength * (dx as f32 * wx + dy as f32 * wy)).max(0.1);
    }
    if same_cell {
        weights[4] = 1.0;
    }
    let total: f32 = weights[..n].iter().sum();
    let mut pick = rng.f32() * total;
    let mut i = 0;
    while i + 1 < n && pick >= weights[i] {
        pick -= weights[i];
        i += 1;
    }
    let (dx, dy) = DIRS.get(i).copied().unwrap_or((0, 0));
    // Embers: in strong wind, a downwind spread may jump 2-3 cells.
    let downwind = dx as f32 * wx + dy as f32 * wy > 0.5;
    if downwind && strength > 0.5 && rng.chance((strength - 0.5) * 0.4) {
        let k = 2 + rng.below(2) as i32;
        return (dx * k, dy * k);
    }
    (dx, dy)
}

fn apply(world: &mut World, content: &Content, rng: &mut Rng, r: &ReactionRt, x: i32, y: i32, env: &SimEnv) -> bool {
    if env.temp < r.temp.0 || env.temp > r.temp.1 {
        return false;
    }
    let mut chance = r.chance;
    if r.wet != 1.0 && env.wet > 0.0 {
        // Things under a roof or canopy stay mostly dry.
        let sheltered = r.layer != Layer::Overlay && world.get(x, y, Layer::Overlay) != NONE_MAT;
        let wet = if sheltered { env.wet * 0.2 } else { env.wet };
        chance *= 1.0 + (r.wet - 1.0) * wet;
    }
    if !rng.chance(chance.min(1.0)) {
        return false;
    }
    let mut other = None;
    if let Some((nl, mask, same)) = &r.neighbor {
        // Four neighbours, plus this same cell on the other layer when allowed.
        let (dx, dy) = pick_dir(rng, r, *same && *nl != r.layer, env);
        let (nx, ny) = (x + dx, y + dy);
        if !world.is_loaded(nx, ny) {
            return false;
        }
        let m = world.get(nx, ny, *nl);
        if !mask.get(m as usize).copied().unwrap_or(false) {
            return false;
        }
        other = Some((nx, ny, *nl));
    }
    if r.swap {
        if let Some((nx, ny, nl)) = other {
            let a = world.get(x, y, r.layer);
            let b = world.get(nx, ny, nl);
            world.set_raw(content, x, y, r.layer, b, NO_OBJ);
            world.set_raw(content, nx, ny, nl, a, NO_OBJ);
        }
    }
    for e in &r.effects {
        let (tx, ty, tl) = match e.target {
            Target::Me => (x, y, r.layer),
            Target::Other => match other {
                Some(o) => o,
                None => continue,
            },
        };
        let l = e.layer.unwrap_or(tl);
        world.set(content, tx, ty, l, e.mat, NO_OBJ);
    }
    true
}

pub fn tick(world: &mut World, content: &Content, rng: &mut Rng, settings: &SimSettings, env: &SimEnv) {
    let rx = &content.reaction_rt;
    let mut keys: Vec<ChunkPos> = world.chunks.keys().copied().collect();
    keys.sort();

    // Active cells.
    let mut budget = settings.max_active;
    for cp in &keys {
        let Some(chunk) = world.chunks.get_mut(cp) else { continue };
        if chunk.active.is_empty() {
            continue;
        }
        let list = std::mem::take(&mut chunk.active);
        for &(i, l) in &list {
            chunk.active_flag[i as usize] &= !(1u8 << l);
        }
        let (ox, oy) = cp.origin();
        let mut keep = Vec::new();
        for (i, l) in list {
            if budget == 0 {
                keep.push((i, l));
                continue;
            }
            budget -= 1;
            let layer = Layer::from_index(l as usize);
            let (x, y) = (ox + (i as i32 % CHUNK), oy + (i as i32 / CHUNK));
            let m = world.get(x, y, layer);
            let Some(list) = rx.active[layer.index()].get(m as usize) else { continue };
            if list.is_empty() {
                continue;
            }
            for &ri in list {
                if world.get(x, y, layer) != m {
                    break;
                }
                apply(world, content, rng, &rx.list[ri], x, y, env);
            }
            // Still active? Keep it in the list.
            let now = world.get(x, y, layer);
            if now != NONE_MAT && rx.is_active(layer, now) {
                keep.push((i, l));
            }
        }
        if let Some(chunk) = world.chunks.get_mut(cp) {
            for (i, l) in keep {
                chunk.mark_active(i as usize, Layer::from_index(l as usize));
            }
        }
    }

    // Random ticks. Layers with no random reactions at all are skipped.
    let layers: Vec<Layer> = Layer::ALL.into_iter().filter(|l| rx.random[l.index()].iter().any(|v| !v.is_empty())).collect();
    if layers.is_empty() {
        return;
    }
    let mut hits: Vec<(i32, i32, Layer, MatId)> = Vec::new();
    for cp in &keys {
        let (ox, oy) = cp.origin();
        // Read the chunk directly (no per-cell map lookup), then react.
        hits.clear();
        if let Some(chunk) = world.chunks.get(cp) {
            for _ in 0..settings.random_samples {
                let i = rng.below(CHUNK_CELLS);
                for &layer in &layers {
                    let m = chunk.mat[layer.index()][i];
                    if rx.random[layer.index()].get(m as usize).is_some_and(|v| !v.is_empty()) {
                        hits.push((ox + i as i32 % CHUNK, oy + i as i32 / CHUNK, layer, m));
                    }
                }
            }
        }
        for &(x, y, layer, m) in &hits {
            if world.get(x, y, layer) != m {
                continue; // changed by an earlier reaction this tick
            }
            for &ri in &rx.random[layer.index()][m as usize] {
                if apply(world, content, rng, &rx.list[ri], x, y, env) {
                    break;
                }
            }
        }
    }
}
