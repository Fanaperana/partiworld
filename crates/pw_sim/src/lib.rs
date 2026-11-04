//! Particle reactions, driven entirely by `ReactionDef` data.
//! Active reactions run every tick on cells that hold an active material
//! (fire). Random reactions run on a few sampled cells per chunk per tick
//! (grass regrowth, ash turning to dirt).

use pw_core::*;
use pw_world::*;

const DIRS: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

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

fn apply(world: &mut World, content: &Content, rng: &mut Rng, r: &ReactionRt, x: i32, y: i32) -> bool {
    if !rng.chance(r.chance) {
        return false;
    }
    let mut other = None;
    if let Some((nl, mask, same)) = &r.neighbor {
        let mut opts: Vec<(i32, i32)> = DIRS.to_vec();
        if *same && *nl != r.layer {
            opts.push((0, 0));
        }
        let (dx, dy) = opts[rng.below(opts.len())];
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

pub fn tick(world: &mut World, content: &Content, rng: &mut Rng, settings: &SimSettings) {
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
                apply(world, content, rng, &rx.list[ri], x, y);
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

    // Random ticks.
    for cp in &keys {
        let (ox, oy) = cp.origin();
        for _ in 0..settings.random_samples {
            let i = rng.below(CHUNK_CELLS) as i32;
            let (x, y) = (ox + i % CHUNK, oy + i / CHUNK);
            for layer in Layer::ALL {
                let m = world.get(x, y, layer);
                if let Some(list) = rx.random[layer.index()].get(m as usize) {
                    for &ri in list {
                        if apply(world, content, rng, &rx.list[ri], x, y) {
                            break;
                        }
                    }
                }
            }
        }
    }
}
