//! Save games. Only chunks the player changed are stored (unchanged ones
//! regenerate from the seed). Content is referenced by string id tables so a
//! save survives mods being added, removed or reordered.

use std::collections::HashMap;
use std::path::Path;

use pw_colony::{Colony, DesTarget};
use pw_core::*;
use pw_game::Game;
use pw_world::*;
use serde::{Deserialize, Serialize};

const VERSION: u32 = 5;

#[derive(Serialize, Deserialize)]
struct Tables {
    materials: Vec<String>,
    objects: Vec<String>,
    items: Vec<String>,
    jobs: Vec<String>,
    creatures: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct SaveFile {
    version: u32,
    seed: u64,
    tick: u64,
    rng: Rng,
    tables: Tables,
    /// lz4(bincode(Chunk)) for every modified chunk.
    chunks: Vec<(ChunkPos, Vec<u8>)>,
    objects: ObjectStore,
    colony: Colony,
    env: pw_game::Env,
    life: pw_life::Life,
    tribes: pw_tribe::Tribes,
    home: TilePos,
}

pub fn save(game: &Game, path: &Path) -> Result<usize, String> {
    let c = &game.content;
    let tables = Tables {
        materials: c.materials.ids(),
        objects: c.objects.ids(),
        items: c.items.ids(),
        jobs: c.jobs.ids(),
        creatures: c.creatures.ids(),
    };
    let mut chunks = Vec::new();
    let mut keys: Vec<&ChunkPos> = game.world.chunks.iter().filter(|(_, ch)| ch.modified).map(|(k, _)| k).collect();
    keys.sort();
    for k in keys {
        let raw = bincode::serialize(&game.world.chunks[k]).map_err(|e| e.to_string())?;
        chunks.push((*k, lz4_flex::compress_prepend_size(&raw)));
    }
    let n = chunks.len();
    let file = SaveFile {
        version: VERSION,
        seed: game.seed,
        tick: game.tick,
        rng: game.rng.clone(),
        tables,
        chunks,
        objects: game.world.objects.clone(),
        colony: game.colony.clone(),
        env: game.env.clone(),
        life: game.life.clone(),
        tribes: game.tribes.clone(),
        home: game.home,
    };
    let bytes = bincode::serialize(&file).map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    // Write to a temp file then rename, so a crash never leaves a half save.
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    Ok(n)
}

/// old index -> new index (None if that content no longer exists)
fn remap(old: &[String], lookup: impl Fn(&str) -> Option<usize>) -> Vec<Option<usize>> {
    old.iter().map(|id| lookup(id)).collect()
}

pub fn load(game: &mut Game, path: &Path) -> Result<Vec<String>, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let file: SaveFile = bincode::deserialize(&bytes).map_err(|e| format!("not a valid save: {e}"))?;
    if file.version != VERSION {
        return Err(format!("save version {} is not supported", file.version));
    }
    let c = game.content.clone();
    let mut warnings = Vec::new();
    let mats = remap(&file.tables.materials, |id| c.mat(id).map(|m| m as usize));
    let objs = remap(&file.tables.objects, |id| c.objects.idx(id));
    let items = remap(&file.tables.items, |id| c.items.idx(id));
    let jobs = remap(&file.tables.jobs, |id| c.jobs.idx(id));
    let creatures = remap(&file.tables.creatures, |id| c.creatures.idx(id));
    for (name, table, map) in [
        ("material", &file.tables.materials, &mats),
        ("object", &file.tables.objects, &objs),
        ("item", &file.tables.items, &items),
    ] {
        for (i, m) in map.iter().enumerate() {
            if m.is_none() {
                warnings.push(format!("missing {name} '{}' was removed", table[i]));
            }
        }
    }

    // Objects.
    let mut store = file.objects;
    let mut dead = Vec::new();
    for inst in store.instances.values_mut() {
        match objs.get(inst.def).copied().flatten() {
            Some(d) => inst.def = d,
            None => dead.push(inst.id),
        }
    }
    for id in &dead {
        store.instances.remove(id);
    }

    // Chunks.
    let mut world = World::new(file.seed);
    for (pos, data) in file.chunks {
        let raw = lz4_flex::decompress_size_prepended(&data).map_err(|e| e.to_string())?;
        let mut ch: Chunk = bincode::deserialize(&raw).map_err(|e| e.to_string())?;
        ch.pos = pos;
        for l in 0..3 {
            for i in 0..CHUNK_CELLS {
                let m = ch.mat[l][i] as usize;
                ch.mat[l][i] = mats.get(m).copied().flatten().unwrap_or(0) as MatId;
                let o = ch.obj[l][i];
                if o != NO_OBJ && !store.instances.contains_key(&o) {
                    ch.obj[l][i] = NO_OBJ;
                }
            }
        }
        ch.after_load(&c);
        ch.modified = true;
        world.chunks.insert(pos, ch);
    }
    world.objects = store;

    let map = Maps { items: &items, jobs: &jobs, objs: &objs, creatures: &creatures };
    let mut colony = file.colony;
    remap_colony(&mut colony, &c, &map);
    let mut tribes = file.tribes;
    for t in &mut tribes.list {
        remap_colony(&mut t.colony, &c, &map);
    }
    tribes.list.retain(|t| c.tribes.get(&t.def_id).is_some() || t.gone.is_some());
    tribes.after_load();

    game.replace_state(world, colony, file.seed, file.tick, file.rng);
    game.env = file.env;
    game.tribes = tribes;
    game.home = file.home;
    // Wild creatures: find their def again by their species' creature id.
    let mut life = file.life;
    let defs: HashMap<u32, Option<usize>> = life.species.iter().map(|s| (s.id, c.creatures.idx(&s.def_id))).collect();
    life.creatures.retain_mut(|cr| match defs.get(&cr.species).copied().flatten() {
        Some(d) => {
            cr.def = d;
            cr.path.clear();
            true
        }
        None => false,
    });
    game.life = life;
    Ok(warnings)
}

/// Old -> new content indices.
struct Maps<'a> {
    items: &'a [Option<usize>],
    jobs: &'a [Option<usize>],
    objs: &'a [Option<usize>],
    creatures: &'a [Option<usize>],
}

/// Point a loaded colony (the player's or a tribe's) at the current content.
fn remap_colony(colony: &mut Colony, c: &Content, m: &Maps) {
    let (items, jobs, objs, creatures) = (m.items, m.jobs, m.objs, m.creatures);
    let item = |i: usize| items.get(i).copied().flatten();
    colony.items.stacks.retain(|_, s| match item(s.item) {
        Some(n) => {
            s.item = n;
            true
        }
        None => false,
    });
    let des: Vec<_> = std::mem::take(&mut colony.designations).into_iter().collect();
    for (t, mut d) in des {
        let Some(j) = jobs.get(d.job).copied().flatten() else { continue };
        d.job = j;
        if let DesTarget::Plant(p) = d.target {
            match objs.get(p).copied().flatten() {
                Some(n) => d.target = DesTarget::Plant(n),
                None => continue,
            }
        }
        colony.designations.insert(t, d);
    }
    colony.blueprints.retain(|_, b| match objs.get(b.def).copied().flatten() {
        Some(n) => {
            b.def = n;
            let len = c.protos[n].build.as_ref().map(|x| x.cost.len()).unwrap_or(0);
            b.delivered.resize(len, 0);
            b.incoming = vec![0; len];
            b.reserved = None;
            true
        }
        None => false,
    });
    for p in &mut colony.pawns {
        p.creature = creatures.get(p.creature).copied().flatten().unwrap_or(0);
        p.carrying = p.carrying.and_then(|(i, n)| item(i).map(|i| (i, n)));
        let g: HashMap<usize, u32> = p.gathered.iter().filter_map(|(i, n)| item(*i).map(|i| (i, *n))).collect();
        p.gathered = g.into_iter().collect();
        // Jobs restart after loading; reservations are cleared below.
        p.job = None;
        p.path.clear();
    }
    for s in colony.items.stacks.values_mut() {
        s.reserved = None;
    }
    for d in colony.designations.values_mut() {
        d.reserved = None;
    }
    colony.after_load();
}
