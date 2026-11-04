//! Partiworld: a top-down colony sim where everything is made of particles.

use std::path::PathBuf;
use std::sync::Arc;

use bevy::prelude::*;
use bevy::render::view::window::screenshot::{Screenshot, save_to_disk};
use bevy_egui::EguiPlugin;
use pw_game::{DAY_TICKS, Extensions, Game, PwPlugin};
use pw_render::{GameRes, PwRenderPlugin};
use pw_ui::PwUiPlugin;

/// Example Rust mod: announces each new day. Rust mods can add any per-tick
/// logic or new script functions; data-only mods need no Rust at all.
struct DayAnnouncer;

impl PwPlugin for DayAnnouncer {
    fn id(&self) -> &str {
        "day_announcer"
    }
    fn build(&self, ext: &mut Extensions) {
        ext.add_tick_system("announce_day", |g: &mut Game| {
            if g.tick > 0 && g.tick % DAY_TICKS == 0 {
                let day = g.tick / DAY_TICKS + 1;
                g.message(format!("Day {day} begins."));
            }
        });
        ext.script_setup.push(Box::new(|engine| {
            engine.register_fn("day_length", || DAY_TICKS as i64);
        }));
    }
}

fn find_mods_dir() -> PathBuf {
    let candidates = [
        PathBuf::from("assets/mods"),
        std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("assets/mods"))).unwrap_or_default(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/mods"),
    ];
    candidates.into_iter().find(|p| p.join("core/mod.toml").exists()).unwrap_or_else(|| PathBuf::from("assets/mods"))
}

fn main() {
    let mods_dir = find_mods_dir();
    let (content, scripts, report) = pw_mods::load_all(&mods_dir);
    for m in &report.loaded {
        println!("loaded mod {m}");
    }
    for w in &report.warnings {
        eprintln!("warning: {w}");
    }
    if !report.ok() {
        for e in &report.errors {
            eprintln!("error: {e}");
        }
        eprintln!("Fix the mod errors above and start again.");
        std::process::exit(1);
    }
    let seed = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(1337);
    let plugins: Vec<Box<dyn PwPlugin>> = vec![Box::new(DayAnnouncer)];
    let ext = Arc::new(Extensions::from_plugins(&plugins));
    let game = Game::new(Arc::new(content), scripts, ext, seed, mods_dir);

    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window { title: "Partiworld".into(), ..default() }),
            ..default()
        }))
        .add_plugins(EguiPlugin::default())
        .insert_resource(GameRes(game))
        .add_plugins((PwRenderPlugin, PwUiPlugin))
        .add_systems(Update, autoshot.run_if(|| std::env::var("PW_AUTOSHOT").is_ok()))
        .add_systems(Update, frame_stats.run_if(|| std::env::var("PW_FRAMES").is_ok()))
        .run();
}

/// Testing aid: `PW_AUTOSHOT=out.png` places a few orders, lets the colony
/// work for a while, saves a screenshot and quits.
fn autoshot(
    mut commands: Commands,
    time: Res<Time>,
    mut game: ResMut<GameRes>,
    mut step: Local<u8>,
    mut ui: ResMut<pw_ui::UiState>,
    mut exit: MessageWriter<AppExit>,
    mut clock: ResMut<pw_render::SimClock>,
) {
    let t = time.elapsed_secs();
    if let Some(s) = std::env::var("PW_AUTOSHOT_SPEED").ok().and_then(|s| s.parse().ok()) {
        clock.speed = s;
    }
    let g = &mut game.0;
    if *step == 0 && t > 1.0 {
        *step = 1;
        ui.show_help = false;
        let home = g.spawn_tile();
        let r = |a: (i32, i32), b: (i32, i32)| pw_core::TileRect::from_corners(home.offset(a.0, a.1), home.offset(b.0, b.1));
        g.designate("core:chop", r((-14, -10), (14, 10)), None);
        g.designate("core:harvest", r((-14, -10), (14, 10)), None);
        g.stockpile(r((-6, -6), (-3, -3)), true);
        g.build("core:wall_wood", r((3, 2), (8, 6)));
        g.build("core:floor_wood", r((4, 3), (7, 5)));
        g.build("core:roof_thatch", r((3, 2), (8, 6)));
        // Optional: start at a given hour to check day/night.
        let day = std::env::var("PW_AUTOSHOT_DAY").ok().and_then(|s| s.parse::<u64>().ok()).unwrap_or(1).max(1) - 1;
        let hour = std::env::var("PW_AUTOSHOT_HOUR").ok().and_then(|s| s.parse::<f32>().ok()).unwrap_or(6.0);
        g.tick = day * pw_game::DAY_TICKS + (((hour - 6.0).rem_euclid(24.0) / 24.0) * pw_game::DAY_TICKS as f32) as u64;
        // Optional: force a weather, fully set in (rain, snow, fog...).
        if let Ok(w) = std::env::var("PW_AUTOSHOT_WEATHER") {
            let climate = g.content.climate.clone();
            if let Some(def) = climate.weather.iter().find(|d| d.id == w) {
                let e = &mut g.env;
                e.weather = def.id.clone();
                e.weather_until = g.tick + 100_000;
                e.precip = def.precipitation;
                e.clouds = def.clouds;
                e.fog = def.fog;
                e.wind = def.wind.1;
                e.wind_target = def.wind.1;
                let s = pw_game::env::season_now(&climate, g.tick);
                e.temp = s.temp + def.temp + e.biome_temp;
                if e.temp < 0.0 && def.precipitation > 0.0 {
                    e.snow = 0.8;
                } else if def.precipitation > 0.0 {
                    e.wetness = 0.8;
                }
            }
        }
        ui.selected = g.colony.pawns.first().map(|p| p.id);
        ui.show_life = std::env::var("PW_AUTOSHOT_LIFE").is_ok();
        // Optional: show off god powers (meteor, lava, water, dig).
        if std::env::var("PW_AUTOSHOT_POWERS").is_ok() {
            let at = |dx: i32, dy: i32| {
                let (x, y) = home.offset(dx, dy).center();
                (x as i32, y as i32)
            };
            let (x, y) = at(-14, -6);
            g.use_power("core:meteor", x, y, 14);
            let (x, y) = at(14, -8);
            g.use_power("core:volcano", x, y, 6);
            for i in 0..12 {
                let (x, y) = at(-16 + i, 8);
                g.use_power("core:dig", x, y, 3);
            }
            let (x, y) = at(-18, 8);
            g.use_power("core:flood", x, y, 6);
        }
        // A finished roof with a colonist under it, to check see-through roofs.
        if std::env::var("PW_AUTOSHOT_ROOF").is_ok() {
            for t in r((-12, 3), (-8, 7)).iter() {
                let (x, y) = t.origin();
                g.place_object("core:roof_thatch", x, y);
            }
            g.spawn_colonist(home.offset(-10, 5));
            if let Some(p) = g.colony.pawns.last_mut() {
                p.drafted = true; // stay put under the roof
            }
        }
        let (x, y) = home.offset(16, 10).center();
        g.ignite(x as i32, y as i32, 4);
        // Optional: let the tribes live N days, then look at the first one.
        if let Some(days) = std::env::var("PW_AUTOSHOT_TRIBES").ok().and_then(|s| s.parse::<u64>().ok()) {
            for _ in 0..days * pw_game::DAY_TICKS {
                g.step();
            }
            if let Some(t) = g.tribes.living().next() {
                let (x, y) = t.center.center();
                ui.goto = Some(Vec2::new(x, y));
                ui.show_tribes = true;
            }
        }
    }
    let shot_at: f32 = std::env::var("PW_AUTOSHOT_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(25.0);
    if *step == 1 && t > shot_at {
        *step = 2;
        let path = std::env::var("PW_AUTOSHOT").unwrap();
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(path));
    }
    if *step == 2 && t > shot_at + 2.0 {
        exit.write(AppExit::Success);
    }
}

/// Testing aid: `PW_FRAMES=1` prints frame time stats every 2 seconds.
fn frame_stats(time: Res<Time>, mut acc: Local<(f32, f32, u32, f32)>, clock: Res<pw_render::SimClock>) {
    let dt = time.delta_secs() * 1000.0;
    acc.0 += dt;
    acc.1 = acc.1.max(dt);
    acc.2 += 1;
    acc.3 += time.delta_secs();
    if acc.3 >= 2.0 {
        println!("FRAMES avg {:.2} ms, worst {:.2} ms, {} frames, speed {}", acc.0 / acc.2 as f32, acc.1, acc.2, clock.speed);
        *acc = (0.0, 0.0, 0, 0.0);
    }
}
