//! Particle-group objects: stamping them into the world, wearing them down,
//! breaking them, and letting them grow.

use pw_core::*;
use pw_world::*;

/// Something that happened to an object; the game turns these into drops and script hooks.
#[derive(Clone, Debug)]
pub enum ObjectEvent {
    Destroyed { def: usize, center: (i32, i32), natural: bool, built: bool },
    Grown { def: usize, center: (i32, i32) },
    Built { id: ObjId, def: usize, center: (i32, i32) },
}

/// Where an object's cells go in the world.
pub fn world_cells(inst_origin: (i32, i32), cells: &[StampCell]) -> impl Iterator<Item = (i32, i32, StampCell)> + '_ {
    cells.iter().map(move |c| (inst_origin.0 + c.dx, inst_origin.1 + c.dy, *c))
}

fn bounds(origin: (i32, i32), cells: &[StampCell]) -> ((i32, i32), (i32, i32)) {
    let mut min = origin;
    let mut max = origin;
    for c in cells {
        let (x, y) = (origin.0 + c.dx, origin.1 + c.dy);
        min = (min.0.min(x), min.1.min(y));
        max = (max.0.max(x), max.1.max(y));
    }
    (min, max)
}

/// Can this object be placed here? Solid cells need empty solid ground and
/// walkable floor; floors need loaded ground.
pub fn can_place(world: &World, content: &Content, def: usize, origin: (i32, i32), seed: u64) -> bool {
    let cells = content.protos[def].cells(seed);
    for (x, y, c) in world_cells(origin, &cells) {
        if !world.is_loaded(x, y) {
            return false;
        }
        if c.layer == Layer::Solid {
            if world.get(x, y, Layer::Solid) != NONE_MAT {
                return false;
            }
            if content.mat_rt(world.get(x, y, Layer::Floor)).walk_cost <= 0.0 {
                return false;
            }
        }
    }
    true
}

/// Create an instance record without stamping particles (construction fills them in over time).
pub fn create_instance(world: &mut World, content: &Content, def: usize, origin: (i32, i32), seed: u64, built: bool) -> ObjId {
    let cells = content.protos[def].cells(seed);
    let (min, max) = bounds(origin, &cells);
    let id = world.objects.alloc();
    world.objects.instances.insert(
        id,
        ObjectInstance {
            id,
            def,
            origin,
            seed,
            total: cells.len() as u32,
            removed: 0,
            min,
            max,
            natural_key: None,
            age: 0,
            built,
        },
    );
    id
}

/// Place an object and stamp all of its particles. Solid cells never overwrite existing solids.
pub fn place(world: &mut World, content: &Content, def: usize, origin: (i32, i32), seed: u64, built: bool) -> ObjId {
    let id = create_instance(world, content, def, origin, seed, built);
    let cells = content.protos[def].cells(seed);
    for (x, y, c) in world_cells(origin, &cells) {
        if c.layer != Layer::Floor && world.get(x, y, c.layer) != NONE_MAT {
            continue;
        }
        world.set(content, x, y, c.layer, c.mat, id);
    }
    id
}

/// All loaded particles still belonging to an object: (x, y, layer).
pub fn live_cells(world: &World, id: ObjId) -> Vec<(i32, i32, Layer)> {
    let Some(inst) = world.objects.get(id) else { return Vec::new() };
    let mut v = Vec::new();
    for y in inst.min.1..=inst.max.1 {
        for x in inst.min.0..=inst.max.0 {
            for l in Layer::ALL {
                if world.get_obj(x, y, l) == id {
                    v.push((x, y, l));
                }
            }
        }
    }
    v
}

/// Remove one random particle of the object (counts as damage). Returns what was removed.
pub fn chip(world: &mut World, content: &Content, id: ObjId, rng: &mut Rng, prefer: Option<Layer>) -> Option<(i32, i32, Layer, MatId)> {
    let cells = live_cells(world, id);
    if cells.is_empty() {
        return None;
    }
    let pool: Vec<_> = match prefer {
        Some(p) if cells.iter().any(|c| c.2 == p) => cells.into_iter().filter(|c| c.2 == p).collect(),
        _ => cells,
    };
    let (x, y, l) = pool[rng.below(pool.len())];
    let m = world.get(x, y, l);
    world.set(content, x, y, l, NONE_MAT, NO_OBJ);
    Some((x, y, l, m))
}

/// Remove one particle of a yield material (e.g. a berry) without damaging the object.
pub fn pluck(world: &mut World, content: &Content, id: ObjId, rng: &mut Rng) -> Option<(i32, i32, MatId)> {
    let cells: Vec<_> = live_cells(world, id)
        .into_iter()
        .filter(|&(x, y, l)| content.mat_rt(world.get(x, y, l)).yields.is_some())
        .collect();
    if cells.is_empty() {
        return None;
    }
    let (x, y, l) = cells[rng.below(cells.len())];
    let m = world.get(x, y, l);
    world.set_raw(content, x, y, l, NONE_MAT, NO_OBJ);
    Some((x, y, m))
}

pub fn has_yield(world: &World, content: &Content, id: ObjId) -> bool {
    live_cells(world, id).iter().any(|&(x, y, l)| content.mat_rt(world.get(x, y, l)).yields.is_some())
}

/// Centre of an object's bounds.
pub fn center(inst: &ObjectInstance) -> (i32, i32) {
    ((inst.min.0 + inst.max.0) / 2, (inst.min.1 + inst.max.1) / 2)
}

/// Remove an object and every particle it still has.
pub fn destroy(world: &mut World, content: &Content, id: ObjId) -> Option<ObjectEvent> {
    let cells = live_cells(world, id);
    for (x, y, l) in cells {
        world.set_raw(content, x, y, l, NONE_MAT, NO_OBJ);
    }
    let inst = world.objects.instances.remove(&id)?;
    world.damaged.remove(&id);
    if let Some(k) = inst.natural_key {
        world.objects.natural.remove(&k);
        world.objects.destroyed_natural.insert(k);
    }
    Some(ObjectEvent::Destroyed { def: inst.def, center: center(&inst), natural: inst.natural_key.is_some(), built: inst.built })
}

/// Break every damaged object that has lost too much.
pub fn process_damage(world: &mut World, content: &Content) -> Vec<ObjectEvent> {
    let ids: Vec<ObjId> = world.damaged.drain().collect();
    let mut out = Vec::new();
    for id in ids {
        let Some(inst) = world.objects.get(id) else { continue };
        let frac = content.protos[inst.def].break_fraction;
        if inst.removed as f32 >= inst.total as f32 * frac {
            if let Some(e) = destroy(world, content, id) {
                out.push(e);
            }
        }
    }
    out
}

/// Slow object life: growth into the next stage and regrowing yield particles.
/// Runs on a slice of objects each tick so cost stays flat.
pub fn tick_life(world: &mut World, content: &Content, tick: u64, rng: &mut Rng) -> Vec<ObjectEvent> {
    const EVERY: u64 = 30;
    let mut out = Vec::new();
    let ids: Vec<ObjId> = world
        .objects
        .instances
        .iter()
        .filter(|(id, _)| (**id as u64 + tick) % EVERY == 0)
        .map(|(id, _)| *id)
        .collect();
    for id in ids {
        let Some(inst) = world.objects.instances.get_mut(&id) else { continue };
        inst.age = inst.age.saturating_add(EVERY as u32);
        let (def, age, origin, seed) = (inst.def, inst.age, inst.origin, inst.seed);
        if !world.is_loaded(origin.0, origin.1) {
            continue;
        }
        let proto = &content.protos[def];
        if let Some((next, ticks)) = proto.growth {
            if age >= ticks {
                // Grow when the origin is loaded; cells blocked by neighbours are simply skipped.
                let fits = world.is_loaded(origin.0, origin.1);
                if fits {
                    let c = center(world.objects.get(id).unwrap());
                    destroy_quiet(world, content, id);
                    place(world, content, next, origin, seed, false);
                    out.push(ObjectEvent::Grown { def: next, center: c });
                }
                continue;
            }
        }
        if let Some(rg) = &proto.regrow {
            if (tick / EVERY) % (rg.interval as u64 / EVERY).max(1) == 0 {
                let cells = proto.cells(seed);
                for (x, y, c) in world_cells(origin, &cells) {
                    if rg.mats.contains(&c.mat) && world.get(x, y, c.layer) == NONE_MAT && rng.chance(rg.chance) {
                        world.set_raw(content, x, y, c.layer, c.mat, id);
                    }
                }
            }
        }
    }
    out
}

/// Remove without marking natural keys destroyed-by-player (used for growth).
fn destroy_quiet(world: &mut World, content: &Content, id: ObjId) {
    for (x, y, l) in live_cells(world, id) {
        world.set_raw(content, x, y, l, NONE_MAT, NO_OBJ);
    }
    if let Some(inst) = world.objects.instances.remove(&id) {
        if let Some(k) = inst.natural_key {
            world.objects.natural.remove(&k);
            world.objects.destroyed_natural.insert(k);
        }
    }
}

/// A natural object proposed by world generation for a chunk.
#[derive(Clone, Debug)]
pub struct NaturalObject {
    pub key: u64,
    pub def: usize,
    pub origin: (i32, i32),
    pub seed: u64,
}

/// Generated chunk: object arrays hold (index into `objects`) + 1 until integrated.
pub struct GeneratedChunk {
    pub chunk: Chunk,
    pub objects: Vec<NaturalObject>,
}

/// Add a freshly generated chunk to the world, linking its natural objects to
/// (possibly already existing) instances and dropping destroyed ones.
pub fn integrate(world: &mut World, content: &Content, mut g: GeneratedChunk) {
    let mut map: Vec<ObjId> = Vec::with_capacity(g.objects.len());
    for n in &g.objects {
        if world.objects.destroyed_natural.contains(&n.key) {
            map.push(NO_OBJ);
            continue;
        }
        let id = match world.objects.natural.get(&n.key) {
            Some(&id) => id,
            None => {
                let id = create_instance(world, content, n.def, n.origin, n.seed, false);
                world.objects.instances.get_mut(&id).unwrap().natural_key = Some(n.key);
                world.objects.natural.insert(n.key, id);
                id
            }
        };
        map.push(id);
    }
    for l in 0..3 {
        for i in 0..CHUNK_CELLS {
            let local = g.chunk.obj[l][i];
            if local != NO_OBJ {
                let id = map[(local - 1) as usize];
                g.chunk.obj[l][i] = id;
                if id == NO_OBJ && l != 0 {
                    g.chunk.mat[l][i] = NONE_MAT;
                }
            }
        }
    }
    g.chunk.modified = false;
    g.chunk.scan_active(content);
    g.chunk.rebuild_tiles(content);
    world.insert_chunk(g.chunk);
}
