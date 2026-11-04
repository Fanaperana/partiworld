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
    let wall = g.content.objects.idx("core:wall_wood").unwrap();
    let built = g.world.objects.instances.values().filter(|i| i.built && i.def == wall && rect.contains(i.origin_tile())).count();
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
    // Dry, still weather so the test is about fire, not rain.
    g.env.weather = "core:clear".into();
    g.env.weather_until = u64::MAX;
    g.env.precip = 0.0;
    g.env.wetness = 0.0;
    g.env.wind_target = 0.0;
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

/// A one-chunk world filled with one floor material, for physics tests.
fn flat_world(c: &Content, floor: &str) -> pw_world::World {
    let mut w = pw_world::World::new(1);
    let f = c.mat(floor).unwrap();
    let mut ch = pw_world::Chunk::new(ChunkPos::new(0, 0));
    ch.mat[0].iter_mut().for_each(|m| *m = f);
    w.insert_chunk(ch);
    w
}

fn count(w: &pw_world::World, c: &Content, layer: Layer, mat: &str) -> usize {
    let m = c.mat(mat).unwrap();
    w.chunks.values().map(|ch| ch.mat[layer.index()].iter().filter(|&&x| x == m).count()).sum()
}

fn sim(w: &mut pw_world::World, c: &Content, ticks: u32, temp: f32) {
    let mut rng = Rng::new(3);
    let env = pw_sim::SimEnv { wet: 0.0, wind: (0.0, 0.0), temp };
    for _ in 0..ticks {
        pw_sim::tick(w, c, &mut rng, &Default::default(), &env);
    }
}

#[test]
fn every_power_works() {
    let mut g = new_game(5);
    let home = g.spawn_tile();
    let ids: Vec<String> = g.content.powers.iter().map(|(_, p)| p.id.clone()).collect();
    assert!(ids.len() >= 10, "{ids:?}");
    for (i, id) in ids.iter().enumerate() {
        let (x, y) = home.offset(-20 + (i as i32 % 5) * 10, 10 + (i as i32 / 5) * 10).center();
        let r = g.content.powers.get(id).unwrap().radius;
        assert!(g.use_power(id, x as i32, y as i32, r), "{id}");
    }
    run(&mut g, 60, |_| false);
    let bad: Vec<&String> = g.log.iter().filter(|m| m.contains("unknown") || m.contains("failed")).collect();
    assert!(bad.is_empty(), "{bad:?}");
}

#[test]
fn lava_flows_burns_and_cools() {
    let c = new_game(1).content.clone();
    let mut w = flat_world(&c, "core:dirt");
    let lava = c.mat("core:lava").unwrap();
    for dy in -3..=3 {
        for dx in -3..=3 {
            w.set(&c, 128 + dx, 128 + dy, Layer::Floor, lava, pw_world::NO_OBJ);
        }
    }
    sim(&mut w, &c, 400, 15.0);
    let hot = count(&w, &c, Layer::Floor, "core:lava") + count(&w, &c, Layer::Floor, "core:lava_flow") + count(&w, &c, Layer::Floor, "core:lava_edge");
    assert!(hot + count(&w, &c, Layer::Floor, "core:basalt") > 49, "lava did not spread");
    sim(&mut w, &c, 6000, 15.0);
    let still_hot = count(&w, &c, Layer::Floor, "core:lava") + count(&w, &c, Layer::Floor, "core:lava_flow") + count(&w, &c, Layer::Floor, "core:lava_edge");
    assert!(still_hot < 10, "lava did not cool: {still_hot} left");
    assert!(count(&w, &c, Layer::Floor, "core:basalt") > 49);
}

#[test]
fn water_freezes_thaws_and_fills_pits() {
    let c = new_game(1).content.clone();
    let mut w = flat_world(&c, "core:water");
    sim(&mut w, &c, 4000, -10.0);
    let ice = count(&w, &c, Layer::Floor, "core:ice");
    assert!(ice > 20_000, "only {ice} ice");
    sim(&mut w, &c, 4000, 10.0);
    assert!(count(&w, &c, Layer::Floor, "core:ice") < ice / 2, "ice did not melt");

    // A channel dug next to water fills up.
    let mut w = flat_world(&c, "core:dirt");
    let water = c.mat("core:water").unwrap();
    let pit = c.mat("core:pit").unwrap();
    for y in 0..256 {
        w.set(&c, 10, y, Layer::Floor, water, pw_world::NO_OBJ);
    }
    for x in 11..40 {
        w.set(&c, x, 100, Layer::Floor, pit, pw_world::NO_OBJ);
    }
    sim(&mut w, &c, 1500, 15.0);
    assert_eq!(count(&w, &c, Layer::Floor, "core:pit"), 0, "channel did not fill");
}

#[test]
fn smoke_drifts_with_the_wind_and_fades() {
    let c = new_game(1).content.clone();
    let mut w = flat_world(&c, "core:dirt");
    let smoke = c.mat("core:smoke").unwrap();
    for dy in -2..=2 {
        for dx in -2..=2 {
            w.set(&c, 128 + dx, 128 + dy, Layer::Overlay, smoke, pw_world::NO_OBJ);
        }
    }
    let mut rng = Rng::new(3);
    let env = pw_sim::SimEnv { wet: 0.0, wind: (0.9, 0.0), temp: 15.0 };
    for _ in 0..40 {
        pw_sim::tick(&mut w, &c, &mut rng, &Default::default(), &env);
    }
    let ch = &w.chunks[&ChunkPos::new(0, 0)];
    let xs: Vec<f32> = ch.mat[2].iter().enumerate().filter(|(_, m)| **m == smoke).map(|(i, _)| (i % 256) as f32).collect();
    assert!(!xs.is_empty());
    let mean = xs.iter().sum::<f32>() / xs.len() as f32;
    assert!(mean > 131.0, "smoke should drift east, mean x {mean}");
    for _ in 0..2000 {
        pw_sim::tick(&mut w, &c, &mut rng, &Default::default(), &env);
    }
    assert_eq!(count(&w, &c, Layer::Overlay, "core:smoke"), 0, "smoke never faded");
}

#[test]
fn wildlife_lives_breeds_learns_and_hunts() {
    let mut g = new_game(12);
    assert!(g.life.alive() >= 10, "start wildlife: {}", g.life.alive());
    let start: Vec<[f32; pw_life::ACTIONS]> = g.life.creatures.iter().map(|c| c.brain).collect();
    let t = std::time::Instant::now();
    run(&mut g, (DAY_TICKS * 4) as u32, |_| false);
    let gens = g.life.creatures.iter().map(|c| c.generation).max().unwrap_or(0);
    let born: u32 = g.life.species.iter().map(|s| s.born).sum();
    let died: u32 = g.life.species.iter().map(|s| s.died).sum();
    println!("4 days: {} alive, {born} born, {died} died, max generation {gens}, {:.1}s", g.life.alive(), t.elapsed().as_secs_f32());
    assert!(born > 0 && gens >= 1, "nothing was born");
    // Children differ from the species average: genes mutate.
    let young: Vec<&pw_life::Creature> = g.life.creatures.iter().filter(|c| c.generation > 0).collect();
    assert!(young.iter().any(|c| c.genes.iter().any(|&x| (x - 1.0).abs() > 0.05)), "no mutation");
    // Brains changed from experience.
    let learned = g.life.creatures.iter().filter(|c| c.brain.iter().any(|&w| (w - 1.0).abs() > 0.15)).count();
    assert!(learned > 0 || start.is_empty(), "nobody learned anything");
    // Grazing ate grass somewhere: some died of something, rabbits kept eating.
    assert!(died > 0 || g.life.alive() > 0);
}

#[test]
fn wolves_hunt_and_lineages_become_new_species() {
    let mut g = new_game(12);
    // Fast mutation so drift shows up within a short test.
    let mut c = (*g.content).clone();
    for (_, d) in c.creatures.iter().map(|(i, d)| (i, d.clone())).collect::<Vec<_>>() {
        if let Some(mut life) = d.life.clone() {
            life.mutation = 0.35;
            life.breed_every_hours = 6.0;
            life.adult_days = 0.2;
            let mut nd = d.clone();
            nd.life = Some(life);
            c.creatures.insert(nd, "test");
        }
    }
    c.link().unwrap();
    g.set_content(std::sync::Arc::new(c));
    let home = g.spawn_tile();
    g.spawn_group("core:wolf", home.offset(4, 4), 4);
    g.spawn_group("core:rabbit", home.offset(6, 4), 12);
    let ok = run(&mut g, (DAY_TICKS * 3) as u32, |g| g.life.species.len() > 3);
    println!("species: {:?}", g.life.species.iter().map(|s| (&s.name, s.alive, s.born)).collect::<Vec<_>>());
    assert!(ok, "no new species appeared");
    assert!(g.log.iter().any(|m| m.contains("new species")));
    // Give the wolves time to hunt.
    let eaten = |g: &Game| g.life.species.iter().map(|s| s.causes[pw_life::Death::Eaten as usize]).sum::<u32>();
    let caught = run(&mut g, (DAY_TICKS * 2) as u32, |g| eaten(g) > 0);
    assert!(caught, "wolves never caught anything");
}

#[test]
fn forests_spread() {
    let mut g = new_game(3);
    g.load_chunks_now(ChunkPos::new(-1, -1), ChunkPos::new(1, 1));
    let sapling = g.content.objects.idx("core:oak_sapling").unwrap();
    let pine_sapling = g.content.objects.idx("core:pine_sapling").unwrap();
    let sprout = g.content.objects.idx("core:berry_sprout").unwrap();
    let young = |g: &Game| g.world.objects.instances.values().filter(|i| [sapling, pine_sapling, sprout].contains(&i.def)).count();
    let before = young(&g);
    let ok = run(&mut g, (DAY_TICKS * 2) as u32, |g| young(g) > before + 2);
    assert!(ok, "no new saplings: {before} -> {}", young(&g));
}

#[test]
#[ignore]
fn debug_wildlife() {
    let mut g = new_game(12);
    let mut causes: std::collections::BTreeMap<String, u32> = Default::default();
    for hour in 0..(24 * 3) {
        for _ in 0..(DAY_TICKS / 24) {
            let before: Vec<(u32, String)> = g.life.creatures.iter().map(|c| (c.id, g.content.creatures.by_idx(c.def).name.clone())).collect();
            g.step();
            let _ = before;
        }
        if hour % 6 == 0 {
            let s: Vec<String> = g.life.creatures.iter().take(6).map(|c| format!("{}:{:.0}e {} b{:?}", g.content.creatures.by_idx(c.def).name, c.energy, c.activity(), c.brain.map(|b| (b * 10.0).round() / 10.0))).collect();
            println!("h{hour} temp {:.0} alive {} :: {}", g.env.temp, g.life.alive(), s.join(" | "));
        }
    }
    let _ = &mut causes;
}

#[test]
fn world_events_reach_scripts() {
    let mut g = new_game(4);
    run(&mut g, 2, |_| false);
    // Force a storm: the example mod throws lightning every few hours.
    g.env.weather = "core:storm".into();
    g.env.weather_until = g.tick + DAY_TICKS;
    run(&mut g, (HOUR_TICKS * 8) as u32, |_| false);
    assert!(g.log.iter().any(|m| m.contains("thunderstorm")), "on_weather not called: {:?}", g.log);
    assert!(!g.log.iter().any(|m| m.contains("failed")), "{:?}", g.log);
    // Seasons change after a season's days.
    let season_ticks = DAY_TICKS * g.content.climate.days_per_season as u64;
    g.tick = season_ticks - 2;
    run(&mut g, 5, |_| false);
    assert_eq!(g.season_name(), "Summer");
    let info = g.world_info();
    assert!(info.counts.get("core:colonist").copied().unwrap_or(0) >= 3);
}

fn roundtrip<T: serde::Serialize + serde::de::DeserializeOwned + Def>(r: &Registry<T>) {
    for (_, d) in r.iter() {
        let text = pw_mods::to_ron(d).unwrap();
        let back: T = pw_mods::parse_ron(&text).unwrap_or_else(|e| panic!("{}: {e}\n{text}", d.def_id()));
        assert_eq!(back.def_id(), d.def_id());
    }
}

#[test]
fn every_definition_can_be_edited_as_text() {
    let g = new_game(1);
    let c = &g.content;
    roundtrip(&c.materials);
    roundtrip(&c.items);
    roundtrip(&c.objects);
    roundtrip(&c.biomes);
    roundtrip(&c.reactions);
    roundtrip(&c.jobs);
    roundtrip(&c.creatures);
    roundtrip(&c.powers);
    roundtrip(&c.tribes);
    let _: ClimateDef = pw_mods::parse_ron(&pw_mods::to_ron(&c.climate).unwrap()).unwrap();
    let _: WorldGenDef = pw_mods::parse_ron(&pw_mods::to_ron(&c.worldgen).unwrap()).unwrap();
}

#[test]
fn live_content_changes_apply_to_the_running_world() {
    let mut g = new_game(1);
    // Recolour grass and add a brand-new material, live.
    let mut c = (*g.content).clone();
    let mut grass = c.materials.get("core:grass").unwrap().clone();
    grass.color = "#ff00ff".into();
    c.materials.insert(grass, "user");
    let mut new = c.materials.get("core:stone").unwrap().clone();
    new.id = "user:marble".into();
    new.name = "Marble".into();
    c.materials.insert(new, "user");
    c.link().unwrap();
    g.swap_content(c).expect("compatible change");
    let gm = g.content.mat("core:grass").unwrap();
    assert_eq!(g.content.mat_rt(gm).color, [255, 0, 255, 255]);
    assert!(g.content.mat("user:marble").is_some());
    run(&mut g, 10, |_| false);

    // A fresh load without the new material would reorder ids: refused.
    let (fresh, _, _) = pw_mods::load_all(&mods_dir());
    assert!(g.swap_content(fresh).is_err());
    // Reloading from disk into a world that matches it works.
    let mut g = new_game(2);
    assert!(g.reload_mods().is_ok());
    run(&mut g, 10, |_| false);
}

fn tribe_report(g: &Game) -> String {
    let mut s = String::new();
    for t in &g.tribes.list {
        let def = t.def(&g.content).unwrap();
        s += &format!(
            "  {} gen{} people {} beds {} food {:.1}d wood {} stone {} techs {:?} research {:?} focus {} culture {:?} projects {} done {} hunted {} born {} died {} gone {:?}\n",
            t.name,
            t.generation,
            t.members.len(),
            t.beds,
            t.food_days(def, &g.content),
            t.item_total(&g.content, "core:wood"),
            t.item_total(&g.content, "core:stone"),
            t.techs,
            t.research,
            pw_tribe::FOCUS_NAMES[t.focus as usize],
            t.culture.map(|v| (v * 100.0).round() / 100.0),
            t.projects.len(),
            t.projects.iter().filter(|p| p.done).count(),
            t.hunted,
            t.born,
            t.died,
            t.gone,
        );
        let acts: Vec<String> = t.colony.pawns.iter().map(|p| p.activity().to_string()).collect();
        s += &format!("    doing: {acts:?}\n");
    }
    s
}

#[test]
#[ignore]
fn debug_tribes() {
    let mut g = new_game(7);
    println!("start:\n{}", tribe_report(&g));
    let t0 = std::time::Instant::now();
    let days: u64 = std::env::var("PW_DAYS").ok().and_then(|d| d.parse().ok()).unwrap_or(12);
    for day in 1..=days {
        for _ in 0..DAY_TICKS {
            g.step();
        }
        println!("day {day} ({:.1}s):\n{}", t0.elapsed().as_secs_f32(), tribe_report(&g));
    }
    for t in &g.tribes.list {
        println!("{}:\n  {}", t.name, t.chronicle.join("\n  "));
    }
}

#[test]
#[ignore]
fn bench_tribes() {
    for with in [false, true] {
        let mut g = new_game(7);
        if !with {
            g.tribes = Default::default();
        }
        let chunks = g.world.chunks.len();
        let t0 = std::time::Instant::now();
        for _ in 0..3000 {
            g.step();
        }
        let a = t0.elapsed().as_secs_f64() * 1000.0 / 3000.0;
        // Time just the tribes.
        let mut tt = 0.0;
        for _ in 0..1500 {
            let content = g.content.clone();
            let env = pw_tribe::TribeEnv { tick: g.tick, day_ticks: DAY_TICKS, dark: 0.0, walk: 1.0, keep_out: vec![] };
            let t1 = std::time::Instant::now();
            g.tribes.tick(&mut g.world, &content, &mut g.rng, &env, &mut g.life);
            tt += t1.elapsed().as_secs_f64();
            g.tick += 1;
        }
        let t2 = std::time::Instant::now();
        for _ in 0..300 {
            let content = g.content.clone();
            pw_sim::tick(&mut g.world, &content, &mut g.rng, &g.sim, &pw_sim::SimEnv { wet: 0.0, wind: (0.0, 0.0), temp: 15.0 });
        }
        let sim = t2.elapsed().as_secs_f64() * 1000.0 / 300.0;
        let t3 = std::time::Instant::now();
        for _ in 0..300 {
            let content = g.content.clone();
            let env = pw_life::LifeEnv { tick: g.tick, day_ticks: DAY_TICKS, temp: 15.0, dark: 0.0, walk: 1.0 };
            g.life.tick(&mut g.world, &content, &mut g.rng, &env);
        }
        let life = t3.elapsed().as_secs_f64() * 1000.0 / 300.0;
        println!("tribes {with}: chunks {chunks} step {a:.3} ms, tribes alone {:.3} ms, sim {sim:.3} ms, life {life:.3} ms ({} animals)", tt * 1000.0 / 1500.0, g.life.alive());
    }
}

#[test]
fn tribes_settle_build_and_grow() {
    let mut g = new_game(7);
    assert_eq!(g.tribes.alive(), 2, "two tribes should settle near the colony");
    for t in g.tribes.living() {
        assert!(t.center.chebyshev(g.home) >= 30, "tribes keep their distance");
        assert!(t.members.len() >= 4);
    }
    let culture0: Vec<[f32; pw_tribe::FOCI]> = g.tribes.list.iter().map(|t| t.culture).collect();
    run(&mut g, (DAY_TICKS * 2 + 600) as u32, |_| false);
    for (i, t) in g.tribes.list.iter().take(2).enumerate() {
        let story = t.chronicle.join(" | ");
        assert!(story.contains("finished a campfire"), "{}: {story}", t.name);
        assert!(t.beds >= 2, "{} should have built a hut with beds: {story}", t.name);
        assert!(t.gone.is_none());
        // A day's results have shifted what it values.
        let moved: f32 = (0..pw_tribe::FOCI).map(|f| (t.culture[f] - culture0[i][f]).abs()).sum();
        assert!(moved > 0.0, "{} culture never changed", t.name);
    }
    // Their people are drawn and saved with everything else.
    assert!(g.all_pawns().any(|(o, _)| o > 0));
}

#[test]
fn tribes_hunt_research_and_split() {
    let mut g = new_game(11);
    g.tribes = Default::default();
    let id = g.found_tribe("core:forest_folk", None, 10).expect("tribe founded");
    // Enough to eat for the band that leaves, and game to hunt.
    let center = g.tribes.get(id).unwrap().center;
    let content = g.content.clone();
    let berries = content.items.idx("core:berries").unwrap();
    {
        let t = g.tribes.list.iter_mut().find(|t| t.id == id).unwrap();
        t.techs.push("hunting".into());
        t.colony.items.add(&g.world, &content, berries, 30, center.offset(3, 0));
    }
    let before = g.tribes.get(id).unwrap().culture;
    run(&mut g, DAY_TICKS as u32 + 10, |_| false);
    let parent = g.tribes.get(id).unwrap();
    let story = parent.chronicle.join(" | ");
    let child = g.tribes.list.iter().find(|t| t.parent == Some(id)).unwrap_or_else(|| panic!("no daughter tribe: {story}"));
    assert_eq!(child.generation, 1);
    assert!(child.techs.contains(&"hunting".to_string()), "knowledge is passed on");
    assert!(child.members.len() >= 3);
    assert_ne!(child.culture, before, "a daughter culture mutates");
    assert!(parent.members.len() + child.members.len() >= 10);
    // Starve the parent of food: its hunters go out for rabbits.
    let meat = content.items.idx("core:meat").unwrap();
    {
        let t = g.tribes.list.iter_mut().find(|t| t.id == id).unwrap();
        let stacks: Vec<(u32, u32)> = t.colony.items.stacks.values().filter(|s| s.item == berries || s.item == meat).map(|s| (s.id, s.count)).collect();
        for (s, n) in stacks {
            t.colony.items.take(s, n);
        }
        // No foraging either (berry bushes may be close): hunting is the way to eat.
        t.techs.retain(|x| x != "foraging");
    }
    g.spawn_group("core:rabbit", center.offset(6, 6), 6);
    let caught = run(&mut g, DAY_TICKS as u32, |g| g.tribes.get(id).is_some_and(|t| t.hunted > 0));
    assert!(caught, "hungry hunters should catch something: {}", g.tribes.get(id).unwrap().chronicle.join(" | "));
}

/// Set every season to one temperature, with clear skies, so needs tests are steady.
fn fixed_climate(g: &mut Game, temp: f32) {
    let mut c = (*g.content).clone();
    for s in &mut c.climate.seasons {
        s.temp = temp;
        s.swing = 0.0;
    }
    g.set_content(Arc::new(c));
    g.env.biome_temp = 0.0;
    g.env.temp = temp;
    g.env.weather = "core:clear".into();
    g.env.weather_until = u64::MAX;
}

#[test]
fn hungry_colonists_eat_and_starve_without_food() {
    let mut g = new_game(21);
    fixed_climate(&mut g, 20.0);
    let meals = g.meals();
    assert!(meals > 5.0, "colony should land with food: {meals}");
    let id = g.colony.pawns[0].id;
    g.colony.pawns[0].needs.food = 0.1;
    let ate = run(&mut g, 1200, |g| g.colony.pawns.iter().find(|p| p.id == id).is_some_and(|p| p.needs.food > 0.6));
    assert!(ate, "hungry colonist did not eat: {}", g.colony.pawns[0].activity());
    assert!(g.meals() < meals, "food was not used up");

    // Take all the food away: a starving colonist dies.
    let food: Vec<u32> = g.colony.items.stacks.values().filter(|s| g.content.items.by_idx(s.item).nutrition > 0.0).map(|s| s.id).collect();
    for s in food {
        g.colony.items.take(s, u32::MAX);
    }
    let p = g.colony.pawns.iter_mut().find(|p| p.id == id).unwrap();
    p.needs.food = 0.0;
    p.needs.health = 0.05;
    let died = run(&mut g, 2000, |g| g.colony.pawns.iter().all(|p| p.id != id));
    assert!(died, "starving colonist did not die");
    assert!(g.log.iter().any(|m| m.contains("starved to death")), "{:?}", g.log);
    assert!(g.log.iter().any(|m| m.contains("no food") || m.contains("running low")), "{:?}", g.log);
}

#[test]
fn tired_colonists_sleep_in_beds() {
    let mut g = new_game(22);
    fixed_climate(&mut g, 20.0);
    let home = g.spawn_tile();
    clear_area(&mut g, home.offset(-6, -6), home.offset(6, 6));
    let c = g.content.clone();
    let bed = c.objects.idx("core:bed").unwrap();
    let bt = home.offset(3, 3);
    let bed_id = pw_objects::place(&mut g.world, &c, bed, bt.origin(), 1, true);
    g.world.rebuild_tiles(&c);
    let id = g.colony.pawns[0].id;
    g.colony.pawns[0].needs.rest = 0.1;
    let in_bed = run(&mut g, 900, |g| {
        g.colony.pawns.iter().find(|p| p.id == id).is_some_and(|p| {
            p.sleeping() && matches!(&p.job, Some(pw_colony::Job { kind: pw_colony::JobKind::Sleep { bed: Some(b), .. }, .. }) if *b == bed_id)
        })
    });
    assert!(in_bed, "tired colonist did not go to bed: {}", g.colony.pawns[0].activity());
    let woke = run(&mut g, (DAY_TICKS / 2) as u32, |g| g.colony.pawns.iter().find(|p| p.id == id).is_some_and(|p| !p.sleeping()));
    assert!(woke, "colonist never woke up");
    assert!(g.colony.pawns.iter().find(|p| p.id == id).unwrap().needs.rest > 0.9);
}

#[test]
fn cold_colonists_warm_up_by_a_fire() {
    let mut g = new_game(23);
    fixed_climate(&mut g, -12.0);
    let home = g.spawn_tile();
    clear_area(&mut g, home.offset(-12, -12), home.offset(12, 12));
    let c = g.content.clone();
    let env = g.needs_env();
    let open = pw_colony::felt_temp(&g.world, &c, home.offset(8, 0), &env);
    let fire = c.objects.idx("core:campfire").unwrap();
    let ft = home.offset(8, 0);
    pw_objects::place(&mut g.world, &c, fire, ft.origin(), 1, true);
    g.world.rebuild_tiles(&c);
    let by_fire = pw_colony::felt_temp(&g.world, &c, home.offset(7, 0), &env);
    assert!(by_fire > open + 15.0, "a campfire should warm the tiles next to it: {open} -> {by_fire}");

    let id = g.colony.pawns[0].id;
    g.colony.pawns[0].needs.warmth = 0.3;
    let warmed = run(&mut g, 1500, |g| g.colony.pawns.iter().find(|p| p.id == id).is_some_and(|p| p.needs.warmth >= 0.9));
    let p = g.colony.pawns.iter().find(|p| p.id == id).unwrap();
    assert!(warmed, "cold colonist did not warm up: {} warmth {:.2} felt {:.0}", p.activity(), p.needs.warmth, p.needs.felt);
    assert!(p.tile().chebyshev(ft) <= 4, "should have gone to the fire");
}
