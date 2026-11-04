use std::path::PathBuf;
use std::sync::Arc;

use pw_core::*;
use pw_game::*;
use pw_world::NO_OBJ;

fn mods_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/mods")
}

fn new_game(seed: u64) -> Game {
    let (content, scripts, report) = pw_mods::load_all(&mods_dir());
    assert!(report.ok(), "mod errors: {:#?}", report.errors);
    Game::new(Arc::new(content), scripts, Arc::new(Extensions::default()), seed, mods_dir())
}

fn run(g: &mut Game, ticks: u32, until: impl Fn(&Game) -> bool) -> bool {
    for _ in 0..ticks {
        g.step();
        if until(g) {
            return true;
        }
    }
    false
}

#[test]
fn mods_load_and_patch() {
    let (c, _s, report) = pw_mods::load_all(&mods_dir());
    assert!(report.ok(), "{:#?}", report.errors);
    assert!(c.objects.get("core:oak_tree").is_some());
    assert!(c.objects.get("example_crystals:crystal_tree").is_some());
    let forest = c.biomes.get("core:forest").unwrap();
    assert!(forest.scatter.iter().any(|s| s.object == "example_crystals:crystal_tree"), "patch did not append scatter");
    assert!(c.reaction_rt.is_active(Layer::Floor, c.mat("core:fire").unwrap()));
}

#[test]
fn mod_order_and_errors() {
    let m = |id: &str, deps: &[&str]| ModInfo {
        id: id.into(),
        name: id.into(),
        version: String::new(),
        description: String::new(),
        depends: deps.iter().map(|s| s.to_string()).collect(),
        load_after: vec![],
        path: PathBuf::new(),
    };
    let sorted = pw_mods::sort_mods(vec![m("b", &["a"]), m("a", &[])]).unwrap();
    assert_eq!(sorted.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
    assert!(pw_mods::sort_mods(vec![m("a", &["b"]), m("b", &["a"])]).is_err());
    assert!(pw_mods::sort_mods(vec![m("a", &["zzz"])]).is_err());
}

#[test]
fn worldgen_is_deterministic() {
    let (c, _, _) = pw_mods::load_all(&mods_dir());
    let a = pw_worldgen::Generator::new(42, &c).generate(&c, ChunkPos::new(3, -2));
    let b = pw_worldgen::Generator::new(42, &c).generate(&c, ChunkPos::new(3, -2));
    assert_eq!(a.chunk.mat, b.chunk.mat);
    assert_eq!(a.objects.len(), b.objects.len());
    let other = pw_worldgen::Generator::new(43, &c).generate(&c, ChunkPos::new(3, -2));
    assert_ne!(a.chunk.mat, other.chunk.mat);
}

#[test]
fn worldgen_has_variety() {
    let (c, _, _) = pw_mods::load_all(&mods_dir());
    let g = pw_worldgen::Generator::new(7, &c);
    let mut counts = vec![0u32; c.biome_rt.len()];
    for y in -40..40 {
        for x in -40..40 {
            let t = g.terrain_at(&c, x * 64, y * 64);
            counts[t.biome] += 1;
        }
    }
    let present = counts.iter().filter(|&&n| n > 0).count();
    for (i, n) in counts.iter().enumerate() {
        eprintln!("{:<12} {n}", c.biomes.by_idx(c.biome_rt[i].def).id);
    }
    assert!(present >= 5, "only {present} biomes in a 5000x5000 area");
}

#[test]
fn objects_cross_chunk_borders_consistently() {
    let mut g = new_game(11);
    g.load_chunks_now(ChunkPos::new(-3, -3), ChunkPos::new(3, 3));
    // Every object cell must point at a live instance.
    for ch in g.world.chunks.values() {
        for l in 0..3 {
            for &o in &ch.obj[l] {
                if o != NO_OBJ {
                    assert!(g.world.objects.get(o).is_some(), "dangling object id {o}");
                }
            }
        }
    }
    assert!(g.world.objects.instances.len() > 50);
}

#[test]
fn pathfinding_goes_around_walls() {
    let mut g = new_game(5);
    let start = g.spawn_tile();
    // Clear a 12x12 area and build a wall with one gap.
    let c = g.content.clone();
    let grass = c.mat("core:grass").unwrap();
    let stone = c.mat("core:stone").unwrap();
    for t in TileRect::from_corners(start.offset(-6, -6), start.offset(6, 6)).iter() {
        for o in g.world.objects_in_tile(t) {
            pw_objects::destroy(&mut g.world, &c, o);
        }
        for (x, y) in pw_world::World::tile_cells(t) {
            g.world.set(&c, x, y, Layer::Floor, grass, NO_OBJ);
            g.world.set(&c, x, y, Layer::Solid, NONE_MAT, NO_OBJ);
            if t.x == start.x + 2 && t.y != start.y + 5 {
                g.world.set(&c, x, y, Layer::Solid, stone, NO_OBJ);
            }
        }
    }
    g.world.rebuild_tiles(&c);
    let goal = start.offset(4, 0);
    let p = pw_colony::find_path(&g.world, start, |t| t == goal, goal, 5000).expect("path");
    assert!(p.len() > 6, "path should detour, got {}", p.len());
    assert!(p.iter().all(|t| g.world.walkable(*t)));
}

fn nearest_object_with_tag(g: &Game, tag: &str) -> Option<(pw_world::ObjId, TilePos)> {
    let home = g.spawn_tile();
    g.world
        .objects
        .instances
        .values()
        .filter(|i| g.content.protos[i.def].has_tag(tag) && !i.built)
        .filter(|i| g.world.is_loaded(i.origin.0, i.origin.1))
        .min_by_key(|i| i.origin_tile().manhattan(home))
        .map(|i| (i.id, i.origin_tile()))
}

#[test]
fn colonists_chop_a_tree() {
    let mut g = new_game(3);
    g.load_chunks_now(ChunkPos::new(-2, -2), ChunkPos::new(2, 2));
    let wood = g.content.items.idx("core:wood").unwrap();
    let before = g.colony.items.total(wood);
    let (tree, tile) = nearest_object_with_tag(&g, "tree").expect("a tree nearby");
    let n = g.designate("core:chop", TileRect::from_corners(tile, tile), None);
    assert_eq!(n, 1);
    let done = run(&mut g, 30 * 120, |g| g.world.objects.get(tree).is_none());
    assert!(done, "tree was not felled in time");
    assert!(g.colony.items.total(wood) > before, "no wood dropped");
}

#[test]
fn colonists_harvest_berries() {
    let mut g = new_game(3);
    g.load_chunks_now(ChunkPos::new(-2, -2), ChunkPos::new(2, 2));
    let berries = g.content.items.idx("core:berries").unwrap();
    let before = g.colony.items.total(berries);
    let (bush, tile) = nearest_object_with_tag(&g, "harvestable").expect("a berry bush nearby");
    assert!(g.designate("core:harvest", TileRect::from_corners(tile, tile), None) == 1);
    let done = run(&mut g, 30 * 150, |g| g.colony.designations.is_empty());
    assert!(done, "harvest did not finish");
    assert!(g.colony.items.total(berries) > before);
    assert!(g.world.objects.get(bush).is_some(), "harvesting must not destroy the bush");
}

#[test]
fn colonists_build_and_haul() {
    let mut g = new_game(9);
    let home = g.spawn_tile();
    let rect = TileRect::from_corners(home.offset(3, 3), home.offset(5, 5));
    // Clear the build spot.
    let c = g.content.clone();
    for t in TileRect::from_corners(home.offset(2, 2), home.offset(6, 6)).iter() {
        for o in g.world.objects_in_tile(t) {
            pw_objects::destroy(&mut g.world, &c, o);
        }
    }
    g.world.rebuild_tiles(&c);
    let placed = g.build("core:wall_wood", rect);
    assert!(placed >= 6, "placed {placed}");
    let ok = run(&mut g, 30 * 300, |g| g.colony.blueprints.is_empty());
    assert!(ok, "walls not built; {} blueprints left", g.colony.blueprints.len());
    let built = g.world.objects.instances.values().filter(|i| i.built).count();
    assert_eq!(built, placed);
    // Walls block movement.
    assert!(!g.world.walkable(rect.min));

    // Stockpile hauling.
    g.stockpile(TileRect::from_corners(home.offset(-6, -6), home.offset(-4, -4)), true);
    let ok = run(&mut g, 30 * 120, |g| {
        g.colony.items.stacks.values().all(|s| g.colony.stockpile.contains(&s.tile))
    });
    assert!(ok, "items not hauled to stockpile");
}

#[test]
fn colonists_mine_rock() {
    let mut g = new_game(21);
    let home = g.spawn_tile();
    let c = g.content.clone();
    let stone = c.mat("core:stone").unwrap();
    let t = home.offset(3, 0);
    for o in g.world.objects_in_tile(t) {
        pw_objects::destroy(&mut g.world, &c, o);
    }
    for (x, y) in pw_world::World::tile_cells(t) {
        g.world.set(&c, x, y, Layer::Solid, stone, NO_OBJ);
    }
    g.world.rebuild_tiles(&c);
    let item = c.items.idx("core:stone").unwrap();
    let before = g.colony.items.total(item);
    assert_eq!(g.designate("core:mine", TileRect::from_corners(t, t), None), 1);
    let ok = run(&mut g, 30 * 200, |g| g.colony.designations.is_empty());
    assert!(ok, "mining did not finish");
    assert!(g.world.walkable(t));
    assert!(g.colony.items.total(item) > before);
}

#[test]
fn fire_spreads_and_burns_out() {
    let mut g = new_game(4);
    g.load_chunks_now(ChunkPos::new(-2, -2), ChunkPos::new(2, 2));
    let (tree, tile) = nearest_object_with_tag(&g, "tree").unwrap();
    let (x, y) = tile.center();
    g.ignite(x as i32, y as i32, 3);
    let fire = g.content.mat("core:fire").unwrap();
    let burning = |g: &Game| g.world.chunks.values().map(|c| c.mat.iter().flatten().filter(|&&m| m == fire).count()).sum::<usize>();
    run(&mut g, 60, |_| false);
    assert!(burning(&g) > 0);
    run(&mut g, 30 * 60, |g| g.world.objects.get(tree).is_none());
    assert!(g.world.objects.get(tree).is_none(), "tree should burn down");
}

#[test]
fn scripts_run_on_hooks() {
    let mut g = new_game(3);
    g.load_chunks_now(ChunkPos::new(-1, -1), ChunkPos::new(1, 1));
    let shard = g.content.items.idx("example_crystals:shard").unwrap();
    let def = g.content.objects.idx("example_crystals:crystal_tree").unwrap();
    let home = g.spawn_tile();
    let (x, y) = home.offset(0, 4).center();
    g.run_hook(def, "on_destroyed", (x as i32, y as i32));
    assert!(g.colony.items.total(shard) >= 2, "{:?}", g.log);
}

#[test]
fn saplings_grow() {
    let mut g = new_game(8);
    let c = g.content.clone();
    let home = g.spawn_tile();
    let t = home.offset(0, 3);
    for o in g.world.objects_in_tile(t) {
        pw_objects::destroy(&mut g.world, &c, o);
    }
    let (x, y) = t.center();
    assert!(g.place_object("core:berry_sprout", x as i32, y as i32));
    let bush = c.objects.idx("core:berry_bush").unwrap();
    let ok = run(&mut g, 2400 + 200, |g| g.world.objects.instances.values().any(|i| i.def == bush && i.origin == (x as i32, y as i32)));
    assert!(ok, "sprout did not grow");
}

#[test]
#[ignore]
fn debug_fire() {
    let mut g = new_game(4);
    g.load_chunks_now(ChunkPos::new(-2, -2), ChunkPos::new(2, 2));
    let (tree, tile) = nearest_object_with_tag(&g, "tree").unwrap();
    let (x, y) = tile.center();
    g.ignite(x as i32, y as i32, 3);
    let fire = g.content.mat("core:fire").unwrap();
    for s in 0..40 {
        let n: usize = g.world.chunks.values().map(|c| c.mat.iter().flatten().filter(|&&m| m == fire).count()).sum();
        let act: usize = g.world.chunks.values().map(|c| c.active.len()).sum();
        let inst = g.world.objects.get(tree).map(|i| (i.removed, i.total));
        eprintln!("t={s:>3}s fire={n} active={act} tree={inst:?}");
        for _ in 0..30 { g.step(); }
    }
}
