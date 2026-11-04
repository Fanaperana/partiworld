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
) {
    let t = time.elapsed_secs();
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
        let (x, y) = home.offset(16, 10).center();
        g.ignite(x as i32, y as i32, 4);
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
