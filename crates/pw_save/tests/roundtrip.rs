use std::path::PathBuf;
use std::sync::Arc;

use pw_core::*;
use pw_game::*;

#[test]
fn save_load_roundtrip() {
    let mods = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/mods");
    let (content, scripts, report) = pw_mods::load_all(&mods);
    assert!(report.ok());
    let content = Arc::new(content);
    let mut g = Game::new(
        content.clone(),
        scripts,
        Arc::new(Extensions::default()),
        77,
        mods.clone(),
    );
    let home = g.spawn_tile();
    g.build(
        "core:wall_wood",
        TileRect::from_corners(home.offset(3, 3), home.offset(6, 6)),
    );
    g.stockpile(
        TileRect::from_corners(home.offset(-5, -5), home.offset(-3, -3)),
        true,
    );
    let (x, y) = home.offset(0, 8).center();
    g.paint(x as i32, y as i32, 6, Layer::Floor, "core:sand");
    for _ in 0..600 {
        g.step();
    }
    let dir = std::env::temp_dir().join("partiworld_test");
    let path = dir.join("roundtrip.pws");
    let n = pw_save::save(&g, &path).unwrap();
    assert!(n > 0);

    let (c2, s2, _) = pw_mods::load_all(&mods);
    let mut g2 = Game::new(Arc::new(c2), s2, Arc::new(Extensions::default()), 1, mods);
    let warnings = pw_save::load(&mut g2, &path).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(g2.seed, 77);
    assert_eq!(g2.tick, g.tick);
    assert_eq!(g2.colony.pawns.len(), g.colony.pawns.len());
    // Needs come back as they were.
    let food: Vec<f32> = g.colony.pawns.iter().map(|p| p.needs.food).collect();
    assert!(
        food.iter().all(|&f| f < 1.0),
        "needs should have dropped: {food:?}"
    );
    assert_eq!(
        g2.colony
            .pawns
            .iter()
            .map(|p| p.needs.food)
            .collect::<Vec<_>>(),
        food
    );
    assert_eq!(g2.colony.blueprints.len(), g.colony.blueprints.len());
    assert_eq!(g2.colony.stockpile, g.colony.stockpile);
    assert_eq!(
        g2.world.objects.instances.len(),
        g.world.objects.instances.len()
    );
    for (k, ch) in g.world.chunks.iter().filter(|(_, c)| c.modified) {
        assert_eq!(g2.world.chunks[k].mat, ch.mat, "chunk {k:?} differs");
    }
    // The loaded game keeps running.
    for _ in 0..300 {
        g2.step();
    }
}

#[test]
fn tribes_survive_save_and_load() {
    let mods = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/mods");
    let (content, scripts, _) = pw_mods::load_all(&mods);
    let mut g = Game::new(
        Arc::new(content),
        scripts,
        Arc::new(Extensions::default()),
        7,
        mods.clone(),
    );
    for _ in 0..600 {
        g.step();
    }
    assert!(g.tribes.alive() > 0);
    let path = std::env::temp_dir()
        .join("partiworld_test")
        .join("tribes.pws");
    pw_save::save(&g, &path).unwrap();
    let names: Vec<String> = g.tribes.list.iter().map(|t| t.name.clone()).collect();
    let people: usize = g.tribes.list.iter().map(|t| t.colony.pawns.len()).sum();
    let (c2, s2, _) = pw_mods::load_all(&mods);
    let mut h = Game::new(Arc::new(c2), s2, Arc::new(Extensions::default()), 99, mods);
    pw_save::load(&mut h, &path).unwrap();
    assert_eq!(
        h.tribes
            .list
            .iter()
            .map(|t| t.name.clone())
            .collect::<Vec<_>>(),
        names
    );
    assert_eq!(
        h.tribes
            .list
            .iter()
            .map(|t| t.colony.pawns.len())
            .sum::<usize>(),
        people
    );
    assert_eq!(h.home, g.home);
    for _ in 0..300 {
        h.step();
    }
}
