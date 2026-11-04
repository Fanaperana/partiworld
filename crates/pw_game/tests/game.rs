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
fn builders_finish_room_from_outside() {
    let mut g = new_game(9);
    let home = g.spawn_tile();
    let c = g.content.clone();
    for t in TileRect::from_corners(home.offset(-1, -1), home.offset(9, 9)).iter() {
        for o in g.world.objects_in_tile(t) {
            pw_objects::destroy(&mut g.world, &c, o);
        }
    }
    g.world.rebuild_tiles(&c);
    // Lots of other work around, at a lower global priority than building.
    g.designate("core:chop", TileRect::from_corners(home.offset(-20, -20), home.offset(20, 20)), None);
    let room = TileRect::from_corners(home.offset(2, 2), home.offset(8, 8));
    let walls = g.build("core:wall_wood", room);
    assert!(walls >= 20, "placed {walls}");
    let ticks = std::cell::Cell::new(0u32);
    let ok = run(&mut g, 30 * 400, |g| {
        ticks.set(ticks.get() + 1);
        g.colony.blueprints.is_empty()
    });
    assert!(ok, "room not built; {} blueprints left", g.colony.blueprints.len());
    println!("room of {walls} walls built in {} ticks", ticks.get());
    let inside = TileRect::from_corners(home.offset(3, 3), home.offset(7, 7));
    for p in &g.colony.pawns {
        assert!(!inside.contains(p.tile()), "{} got walled in", p.name);
    }
}

#[test]
fn work_priorities_turn_jobs_off() {
    let mut g = new_game(3);
    g.load_chunks_now(ChunkPos::new(-2, -2), ChunkPos::new(2, 2));
    for p in &mut g.colony.pawns {
        p.work.insert("core:chop".into(), 0);
    }
    let (tree, tile) = nearest_object_with_tag(&g, "tree").expect("a tree nearby");
    g.designate("core:chop", TileRect::from_corners(tile, tile), None);
    run(&mut g, 30 * 60, |_| false);
    assert!(g.world.objects.get(tree).is_some(), "tree chopped with chopping turned off");
    assert_eq!(g.world.objects.get(tree).unwrap().removed, 0);
    g.colony.pawns[0].work.insert("core:chop".into(), 1);
    assert!(run(&mut g, 30 * 120, |g| g.world.objects.get(tree).is_none()), "tree not felled once allowed");
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
    // Growth speed depends on season and temperature, so allow some slack.
    let ok = run(&mut g, 2400 * 3, |g| g.world.objects.instances.values().any(|i| i.def == bush && i.origin == (x as i32, y as i32)));
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

/// `cargo test --release -p pw_game --test game bench -- --ignored --nocapture`
#[test]
#[ignore]
fn bench_busy_colony() {
    let mut g = new_game(5);
    let home = g.spawn_tile();
    for i in 0..9 {
        g.spawn_colonist(home.offset(i % 3, i / 3));
    }
    g.load_chunks_now(ChunkPos::new(-3, -3), ChunkPos::new(3, 3));
    let big = TileRect::from_corners(home.offset(-60, -60), home.offset(60, 60));
    let t0 = std::time::Instant::now();
    g.designate("core:chop", big, None);
    g.designate("core:harvest", big, None);
    g.designate("core:quarry", big, None);
    println!("BENCH designate {} orders in {:.1} ms", g.colony.designations.len(), t0.elapsed().as_secs_f64() * 1000.0);
    g.stockpile(TileRect::from_corners(home.offset(-8, -8), home.offset(-3, -3)), true);
    g.build("core:wall_wood", TileRect::from_corners(home.offset(4, 4), home.offset(12, 12)));
    let t = std::time::Instant::now();
    let mut worst = 0f64;
    for _ in 0..3000 {
        let s = std::time::Instant::now();
        g.step();
        worst = worst.max(s.elapsed().as_secs_f64() * 1000.0);
    }
    println!("BENCH worst tick {worst:.2} ms");
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    println!("BENCH {:.3} ms/tick, {} designations left, {} blueprints left", ms / 3000.0, g.colony.designations.len(), g.colony.blueprints.len());
}

fn clear_area(g: &mut Game, a: TilePos, b: TilePos) {
    let c = g.content.clone();
    for t in TileRect::from_corners(a, b).iter() {
        for o in g.world.objects_in_tile(t) {
            pw_objects::destroy(&mut g.world, &c, o);
        }
    }
    g.world.rebuild_tiles(&c);
}

#[test]
fn roofs_need_support_and_collapse_without_it() {
    let mut g = new_game(9);
    let home = g.spawn_tile();
    clear_area(&mut g, home.offset(-2, -2), home.offset(14, 14));
    // Out in the open: no wall or rock nearby.
    let t = home.offset(10, 10);
    assert_eq!(g.build("core:roof_thatch", TileRect::from_corners(t, t)), 0);
    assert!(g.last_build_error.as_deref().unwrap_or("").contains("wall"), "{:?}", g.last_build_error);

    // A finished wall makes roofs nearby possible.
    let c = g.content.clone();
    let wall = c.objects.idx("core:wall_wood").unwrap();
    let wt = home.offset(4, 4);
    let wall_id = pw_objects::place(&mut g.world, &c, wall, wt.origin(), 1, true);
    g.world.rebuild_tiles(&c);
    let rt = home.offset(6, 4);
    assert_eq!(g.build("core:roof_thatch", TileRect::from_corners(rt, rt)), 1);
    // ...and a roof placed straight away stays up.
    let roof = c.objects.idx("core:roof_thatch").unwrap();
    let rt2 = home.offset(7, 4);
    let roof_id = pw_objects::place(&mut g.world, &c, roof, rt2.origin(), 1, true);
    run(&mut g, 90, |_| false);
    assert!(g.world.objects.get(roof_id).is_some(), "supported roof fell");

    // Knock the wall down: the roof falls in.
    pw_objects::destroy(&mut g.world, &c, wall_id);
    g.world.rebuild_tiles(&c);
    let fell = run(&mut g, 90, |g| g.world.objects.get(roof_id).is_none());
    assert!(fell, "roof without support did not collapse");
    assert!(g.log.iter().any(|m| m.contains("collapsed")));
}

#[test]
fn seasons_and_weather_change() {
    let mut g = new_game(4);
    let climate = g.content.climate.clone();
    let year = DAY_TICKS * climate.days_per_season as u64 * climate.seasons.len() as u64;
    let mut seasons = std::collections::BTreeSet::new();
    let mut weathers = std::collections::BTreeSet::new();
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    // Step the climate alone through a whole year (fast).
    let mut rng = Rng::new(1);
    let mut env = g.env.clone();
    for tick in (0..year).step_by(30) {
        for k in 0..30 {
            env.tick(&climate, tick + k, &mut rng);
        }
        seasons.insert(pw_game::env::calendar(&climate, tick).season);
        weathers.insert(env.weather.clone());
        lo = lo.min(env.temp);
        hi = hi.max(env.temp);
    }
    assert_eq!(seasons.len(), climate.seasons.len());
    assert!(weathers.len() >= 4, "{weathers:?}");
    assert!(lo < 0.0 && hi > 15.0, "temperature range {lo}..{hi}");
    // Nights are darker than noon.
    g.tick = DAY_TICKS / 4; // 12:00
    let noon = g.light().dark;
    g.tick = DAY_TICKS * 3 / 4; // 00:00
    assert!(g.light().dark > noon + 0.5);
}

/// Burn a field of dry grass and return (cells burnt, mean x of burnt cells).
fn burn_field(wet: f32, wind: (f32, f32)) -> (usize, f32) {
    let c = new_game(1).content.clone();
    let mut w = pw_world::World::new(1);
    let grass = c.mat("core:dry_grass").unwrap();
    let ash = c.mat("core:ash").unwrap();
    let mut ch = pw_world::Chunk::new(ChunkPos::new(0, 0));
    ch.mat[0].iter_mut().for_each(|m| *m = grass);
    w.insert_chunk(ch);
    let fire = c.mat("core:fire").unwrap();
    for dy in -2..=2 {
        for dx in -2..=2 {
            w.set(&c, 128 + dx, 128 + dy, Layer::Floor, fire, pw_world::NO_OBJ);
        }
    }
    let mut rng = Rng::new(7);
    let env = pw_sim::SimEnv { wet, wind, temp: 15.0 };
    for _ in 0..400 {
        pw_sim::tick(&mut w, &c, &mut rng, &Default::default(), &env);
    }
    let ch = &w.chunks[&ChunkPos::new(0, 0)];
    let (mut n, mut sx) = (0usize, 0f32);
    for (i, &m) in ch.mat[0].iter().enumerate() {
        if m == ash || m == fire {
            n += 1;
            sx += (i % 256) as f32;
        }
    }
    (n, if n > 0 { sx / n as f32 } else { 128.0 })
}

#[test]
fn rain_slows_fire_and_wind_pushes_it() {
    let (dry, _) = burn_field(0.0, (0.0, 0.0));
    let (wet, _) = burn_field(1.0, (0.0, 0.0));
    assert!(wet * 3 < dry, "rain should slow fire: dry {dry}, wet {wet}");
    let (_, east) = burn_field(0.0, (0.9, 0.0));
    let (_, west) = burn_field(0.0, (-0.9, 0.0));
    assert!(east > 128.0 + 4.0 && west < 128.0 - 4.0, "wind should push fire: east {east}, west {west}");
}
