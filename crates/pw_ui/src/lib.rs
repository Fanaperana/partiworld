//! Player-facing UI: camera, mouse tools, panels and the object editor.

mod data_editor;
mod editor;

use std::path::PathBuf;
use std::sync::Arc;

use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_egui::input::{egui_wants_any_keyboard_input, egui_wants_any_pointer_input};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use pw_core::*;
use pw_game::Game;
use pw_render::{GameRes, MainCamera, RenderSettings, SimClock};

pub use data_editor::DataEditor;
pub use editor::Editor;

#[derive(Clone, Debug, PartialEq)]
pub enum Tool {
    Inspect,
    Order(String),
    Cancel,
    Stockpile,
    Unzone,
    Build(String),
    Plant(String),
    Paint,
    Place(String),
    Ignite,
    SpawnColonist,
    /// A god power from data.
    Power(String),
}

impl Tool {
    fn drags(&self) -> bool {
        matches!(self, Tool::Order(_) | Tool::Cancel | Tool::Stockpile | Tool::Unzone | Tool::Build(_) | Tool::Plant(_))
    }
}

#[derive(Resource)]
pub struct UiState {
    pub tool: Tool,
    drag_start: Option<Vec2>,
    pub cursor: Option<Vec2>,
    pub paint_mat: String,
    pub paint_layer: Layer,
    pub paint_radius: i32,
    pub place_def: String,
    pub seed_text: String,
    pub show_editor: bool,
    pub show_help: bool,
    pub show_work: bool,
    pub show_tribes: bool,
    /// Move the camera here once (e.g. "Go to" a tribe).
    pub goto: Option<Vec2>,
    pub show_life: bool,
    pub show_data: bool,
    pub data: DataEditor,
    /// Reload mods from disk when their files change.
    pub live_reload: bool,
    /// Selected colonist (pawn id) and whether the camera follows it.
    pub selected: Option<u32>,
    pub follow: bool,
    /// Last build / plant choice, for the B and T hotkeys.
    pub last_build: String,
    pub last_plant: String,
    /// Brush radius for god powers.
    pub power_radius: i32,
    power_timer: f32,
    pub editor: Editor,
    pub toast: Option<(String, f32)>,
    pub save_path: PathBuf,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            tool: Tool::Inspect,
            drag_start: None,
            cursor: None,
            paint_mat: "core:water".into(),
            paint_layer: Layer::Floor,
            paint_radius: 4,
            place_def: "core:oak_tree".into(),
            seed_text: String::new(),
            show_editor: false,
            show_help: true,
            show_work: false,
            show_tribes: false,
            goto: None,
            show_life: false,
            show_data: false,
            data: DataEditor::default(),
            live_reload: true,
            selected: None,
            follow: false,
            last_build: "core:wall_wood".into(),
            last_plant: "core:oak_sapling".into(),
            power_radius: 6,
            power_timer: 0.0,
            editor: Editor::default(),
            toast: None,
            save_path: PathBuf::from("saves/quicksave.pws"),
        }
    }
}

impl UiState {
    pub fn toast(&mut self, m: impl Into<String>) {
        self.toast = Some((m.into(), 4.0));
    }
}

/// Keyboard shortcuts for tools. Shown next to the tool names.
fn hotkey_tools(ui: &UiState) -> Vec<(KeyCode, &'static str, Tool)> {
    vec![
        (KeyCode::KeyC, "C", Tool::Order("core:chop".into())),
        (KeyCode::KeyM, "M", Tool::Order("core:mine".into())),
        (KeyCode::KeyG, "G", Tool::Order("core:harvest".into())),
        (KeyCode::KeyX, "X", Tool::Cancel),
        (KeyCode::KeyZ, "Z", Tool::Stockpile),
        (KeyCode::KeyB, "B", Tool::Build(ui.last_build.clone())),
        (KeyCode::KeyT, "T", Tool::Plant(ui.last_plant.clone())),
    ]
}

fn hotkey_label(ui: &UiState, tool: &Tool) -> Option<&'static str> {
    hotkey_tools(ui).into_iter().find(|(_, _, t)| t == tool).map(|(_, k, _)| k)
}

/// Colonist nearest to a world point, within `reach` particles.
fn pawn_at(g: &Game, p: Vec2, reach: f32) -> Option<u32> {
    g.colony
        .pawns
        .iter()
        .map(|c| (Vec2::new(c.pos.0, c.pos.1).distance(p), c.id))
        .filter(|(d, _)| *d <= reach)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|x| x.1)
}

pub struct PwUiPlugin;

impl Plugin for PwUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UiState>()
            .add_systems(Startup, init_ui)
            .add_systems(Update, camera_keys.run_if(not(egui_wants_any_keyboard_input)))
            .add_systems(Update, (camera_mouse, world_tools).chain().run_if(not(egui_wants_any_pointer_input)))
            .add_systems(Update, (draw_tool_preview, follow_camera, watch_mods))
            .add_systems(EguiPrimaryContextPass, ui_panels);
    }
}

/// A cheap fingerprint of every file under the mods folder (names, sizes, times).
fn mods_fingerprint(dir: &std::path::Path) -> u64 {
    fn walk(p: &std::path::Path, h: &mut u64) {
        let Ok(rd) = std::fs::read_dir(p) else { return };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.path());
        for e in entries {
            let path = e.path();
            if path.is_dir() {
                walk(&path, h);
            } else if let Ok(m) = e.metadata() {
                let t = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos() as u64).unwrap_or(0);
                *h = splitmix64(*h ^ hash_str(&path.to_string_lossy()) ^ m.len() ^ t);
            }
        }
    }
    let mut h = 0;
    walk(dir, &mut h);
    h
}

/// Hot reload: when a mod file changes on disk, reload mods into the running world.
fn watch_mods(time: Res<Time>, mut ui: ResMut<UiState>, mut game: ResMut<GameRes>, mut state: Local<(f32, u64)>) {
    state.0 -= time.delta_secs();
    if state.0 > 0.0 {
        return;
    }
    state.0 = 1.0;
    let fp = mods_fingerprint(&game.0.mods_dir);
    if state.1 == 0 {
        state.1 = fp;
        return;
    }
    if fp == state.1 {
        return;
    }
    state.1 = fp;
    if !ui.live_reload {
        return;
    }
    match game.0.reload_mods() {
        Ok(m) => ui.toast(format!("{m} (files changed)")),
        Err(e) => ui.toast(format!("Mod reload: {e}")),
    }
}

fn init_ui(mut ui: ResMut<UiState>, game: Res<GameRes>) {
    ui.seed_text = game.0.seed.to_string();
}

fn camera_keys(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut cam: Query<(&mut Transform, &mut Projection), With<MainCamera>>,
    mut clock: ResMut<SimClock>,
    mut settings: ResMut<RenderSettings>,
    mut ui: ResMut<UiState>,
    mut game: ResMut<GameRes>,
    window: Query<&Window, With<PrimaryWindow>>,
) {
    let Ok((mut t, mut p)) = cam.single_mut() else { return };
    let Projection::Orthographic(o) = p.as_mut() else { return };
    let maxz = window.single().map(|w| pw_render::max_zoom(w.width(), w.height())).unwrap_or(2.0);
    let mut d = Vec2::ZERO;
    if keys.any_pressed([KeyCode::KeyW, KeyCode::ArrowUp]) {
        d.y += 1.0;
    }
    if keys.any_pressed([KeyCode::KeyS, KeyCode::ArrowDown]) {
        d.y -= 1.0;
    }
    if keys.any_pressed([KeyCode::KeyA, KeyCode::ArrowLeft]) {
        d.x -= 1.0;
    }
    if keys.any_pressed([KeyCode::KeyD, KeyCode::ArrowRight]) {
        d.x += 1.0;
    }
    if d != Vec2::ZERO {
        ui.follow = false;
    }
    let fast = if keys.pressed(KeyCode::ShiftLeft) { 2.5 } else { 1.0 };
    t.translation += (d * 900.0 * o.scale * fast * time.delta_secs()).extend(0.0);
    if keys.pressed(KeyCode::KeyQ) {
        o.scale = o.scale * (1.0 + 2.0 * time.delta_secs());
    }
    if keys.pressed(KeyCode::KeyE) {
        o.scale = (o.scale / (1.0 + 2.0 * time.delta_secs())).max(0.04);
    }
    // Also re-clamps after the window grows (fullscreen).
    o.scale = o.scale.clamp(0.04, maxz);
    if keys.just_pressed(KeyCode::Space) {
        clock.speed = if clock.speed == 0 { 1 } else { 0 };
    }
    for (k, s) in [(KeyCode::Digit1, 1), (KeyCode::Digit2, 2), (KeyCode::Digit3, 3)] {
        if keys.just_pressed(k) {
            clock.speed = s;
        }
    }
    if keys.just_pressed(KeyCode::KeyR) {
        settings.show_overlay = !settings.show_overlay;
    }
    if keys.just_pressed(KeyCode::KeyV) {
        settings.see_through = !settings.see_through;
    }
    if keys.just_pressed(KeyCode::KeyH) || keys.just_pressed(KeyCode::F1) {
        ui.show_help = !ui.show_help;
    }
    if keys.just_pressed(KeyCode::Escape) {
        if ui.tool == Tool::Inspect {
            ui.selected = None;
            ui.follow = false;
        }
        ui.tool = Tool::Inspect;
        ui.drag_start = None;
    }
    for (k, _, tool) in hotkey_tools(&ui) {
        if keys.just_pressed(k) {
            ui.tool = tool;
            ui.drag_start = None;
        }
    }
    if keys.just_pressed(KeyCode::KeyF) && ui.selected.is_some() {
        ui.follow = !ui.follow;
    }
    if keys.just_pressed(KeyCode::KeyP) {
        ui.show_work = !ui.show_work;
    }
    if keys.just_pressed(KeyCode::KeyL) {
        ui.show_life = !ui.show_life;
    }
    if keys.just_pressed(KeyCode::KeyK) {
        ui.show_tribes = !ui.show_tribes;
    }
    if keys.just_pressed(KeyCode::F2) {
        ui.show_data = !ui.show_data;
    }
    if keys.just_pressed(KeyCode::F5) {
        quick_save(&game.0, &mut ui);
    }
    if keys.just_pressed(KeyCode::F9) {
        quick_load(&mut game.0, &mut ui);
    }
}

fn follow_camera(
    mut ui: ResMut<UiState>,
    game: Res<GameRes>,
    mut cam: Query<&mut Transform, With<MainCamera>>,
    mut settings: ResMut<RenderSettings>,
    clock: Res<SimClock>,
) {
    // The render side fades roofs around the cursor.
    if settings.cursor != ui.cursor {
        settings.cursor = ui.cursor;
    }
    if let Some(at) = ui.goto.take() {
        ui.follow = false;
        if let Ok(mut t) = cam.single_mut() {
            t.translation.x = at.x;
            t.translation.y = at.y;
        }
        return;
    }
    if !ui.follow {
        return;
    }
    let Some(p) = ui.selected.and_then(|id| game.0.colony.pawns.iter().find(|p| p.id == id)) else { return };
    if let Ok(mut t) = cam.single_mut() {
        let d = clock.draw_pos(p);
        t.translation.x = d.x;
        t.translation.y = d.y;
    }
}

fn camera_mouse(
    buttons: Res<ButtonInput<MouseButton>>,
    scroll: Res<AccumulatedMouseScroll>,
    motion: Res<AccumulatedMouseMotion>,
    mut cam: Query<(&mut Transform, &mut Projection), With<MainCamera>>,
    mut ui: ResMut<UiState>,
    window: Query<&Window, With<PrimaryWindow>>,
) {
    let Ok((mut t, mut p)) = cam.single_mut() else { return };
    let Projection::Orthographic(o) = p.as_mut() else { return };
    let maxz = window.single().map(|w| pw_render::max_zoom(w.width(), w.height())).unwrap_or(2.0);
    if scroll.delta.y != 0.0 {
        let f = if scroll.delta.y > 0.0 { 1.0 / 1.15 } else { 1.15 };
        o.scale = (o.scale * f).clamp(0.04, maxz);
    }
    if buttons.pressed(MouseButton::Middle) {
        if motion.delta != Vec2::ZERO {
            ui.follow = false;
        }
        t.translation.x -= motion.delta.x * o.scale;
        t.translation.y += motion.delta.y * o.scale;
    }
}

fn cursor_world(
    window: &Query<&Window, With<PrimaryWindow>>,
    cam: &Query<(&Camera, &GlobalTransform), With<MainCamera>>,
) -> Option<Vec2> {
    let w = window.single().ok()?;
    let (c, gt) = cam.single().ok()?;
    let pos = w.cursor_position()?;
    c.viewport_to_world_2d(gt, pos).ok()
}

fn tile_at(v: Vec2) -> TilePos {
    tile_of_f(v.x, v.y)
}

fn world_tools(
    buttons: Res<ButtonInput<MouseButton>>,
    window: Query<&Window, With<PrimaryWindow>>,
    cam: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    mut ui: ResMut<UiState>,
    mut game: ResMut<GameRes>,
    time: Res<Time>,
) {
    ui.cursor = cursor_world(&window, &cam);
    let Some(cur) = ui.cursor else { return };
    let g = &mut game.0;
    if buttons.just_pressed(MouseButton::Right) {
        ui.tool = Tool::Inspect;
        ui.drag_start = None;
        return;
    }
    let tool = ui.tool.clone();
    if tool.drags() {
        if buttons.just_pressed(MouseButton::Left) {
            ui.drag_start = Some(cur);
        }
        if buttons.just_released(MouseButton::Left) {
            if let Some(start) = ui.drag_start.take() {
                let rect = TileRect::from_corners(tile_at(start), tile_at(cur));
                let msg = match &tool {
                    Tool::Order(job) => {
                        let n = g.designate(job, rect, None);
                        format!("{n} orders placed")
                    }
                    Tool::Plant(def) => {
                        let n = g.designate("core:plant", rect, Some(def));
                        format!("{n} tiles marked for planting")
                    }
                    Tool::Cancel => {
                        g.cancel(rect);
                        "Orders cancelled".into()
                    }
                    Tool::Stockpile => {
                        g.stockpile(rect, true);
                        "Stockpile zone added".into()
                    }
                    Tool::Unzone => {
                        g.stockpile(rect, false);
                        "Zone removed".into()
                    }
                    Tool::Build(def) => {
                        let n = g.build(def, rect);
                        match (n, g.last_build_error.clone()) {
                            (0, Some(e)) => format!("Can't build: {e}"),
                            (0, None) => "Can't build there".into(),
                            (n, Some(e)) if e.contains("roof") => format!("{n} blueprints placed; some skipped: {e}"),
                            (n, _) => format!("{n} blueprints placed"),
                        }
                    }
                    _ => String::new(),
                };
                if !msg.is_empty() {
                    ui.toast(msg);
                }
            }
        }
        return;
    }
    let (x, y) = (cur.x.floor() as i32, cur.y.floor() as i32);
    match tool {
        Tool::Inspect if buttons.just_pressed(MouseButton::Left) => {
            ui.selected = pawn_at(g, cur, 6.0);
            if ui.selected.is_none() {
                ui.follow = false;
            }
        }
        Tool::Paint => {
            if buttons.pressed(MouseButton::Left) {
                let (m, l, r) = (ui.paint_mat.clone(), ui.paint_layer, ui.paint_radius);
                g.paint(x, y, r, l, &m);
            }
        }
        Tool::Place(def) if buttons.just_pressed(MouseButton::Left) => {
            if !g.place_object(&def, x, y) {
                ui.toast("Can't place there");
            }
        }
        Tool::Ignite if buttons.just_pressed(MouseButton::Left) => g.ignite(x, y, 3),
        Tool::Power(id) => {
            let continuous = g.content.powers.get(&id).is_some_and(|p| p.continuous);
            ui.power_timer -= time.delta_secs();
            let fire = if continuous {
                buttons.pressed(MouseButton::Left) && ui.power_timer <= 0.0
            } else {
                buttons.just_pressed(MouseButton::Left)
            };
            if fire {
                ui.power_timer = 0.06;
                g.use_power(&id, x, y, ui.power_radius);
            }
        }
        Tool::SpawnColonist if buttons.just_pressed(MouseButton::Left) => g.spawn_colonist(tile_of(x, y)),
        _ => {}
    }
}

fn draw_tool_preview(ui: Res<UiState>, mut gizmos: Gizmos, game: Res<GameRes>, time: Res<Time>, clock: Res<SimClock>) {
    if let Some(p) = ui.selected.and_then(|id| game.0.colony.pawns.iter().find(|p| p.id == id)) {
        let pulse = 6.5 + (time.elapsed_secs() * 4.0).sin() * 0.4;
        let at = clock.draw_pos(p);
        gizmos.circle_2d(Isometry2d::from_translation(at), pulse, Color::srgb(1.0, 0.95, 0.4));
        if let Some(&t) = p.path.first() {
            let (x, y) = t.center();
            gizmos.line_2d(at, Vec2::new(x, y), Color::srgba(1.0, 0.95, 0.4, 0.35));
        }
    }
    let Some(cur) = ui.cursor else { return };
    let tile = TILE as f32;
    let rect_of = |r: TileRect| {
        let min = Vec2::new((r.min.x * TILE) as f32, (r.min.y * TILE) as f32);
        let max = Vec2::new(((r.max.x + 1) * TILE) as f32, ((r.max.y + 1) * TILE) as f32);
        ((min + max) / 2.0, max - min)
    };
    let color = match &ui.tool {
        Tool::Order(job) => game
            .0
            .content
            .jobs
            .get(job)
            .and_then(|j| parse_color(&j.color))
            .map(|c| Color::srgb_u8(c[0], c[1], c[2]))
            .unwrap_or(Color::WHITE),
        Tool::Cancel => Color::srgb(1.0, 0.3, 0.3),
        Tool::Stockpile | Tool::Unzone => Color::srgb(1.0, 0.85, 0.3),
        Tool::Build(_) => Color::srgb(0.4, 0.6, 1.0),
        Tool::Plant(_) => Color::srgb(0.4, 1.0, 0.6),
        _ => Color::srgba(1.0, 1.0, 1.0, 0.5),
    };
    match (&ui.tool, ui.drag_start) {
        (Tool::Paint, _) => {
            gizmos.circle_2d(Isometry2d::from_translation(cur), ui.paint_radius as f32 + 0.5, Color::WHITE);
        }
        (Tool::Power(_), _) => {
            gizmos.circle_2d(Isometry2d::from_translation(cur), ui.power_radius as f32 + 0.5, Color::srgb(1.0, 0.8, 0.4));
        }
        (Tool::Ignite, _) => {
            gizmos.circle_2d(Isometry2d::from_translation(cur), 3.5, Color::srgb(1.0, 0.5, 0.1));
        }
        (_, Some(start)) => {
            let (c, s) = rect_of(TileRect::from_corners(tile_at(start), tile_at(cur)));
            gizmos.rect_2d(Isometry2d::from_translation(c), s, color);
        }
        _ => {
            let t = tile_at(cur);
            let (c, _) = rect_of(TileRect::from_corners(t, t));
            gizmos.rect_2d(Isometry2d::from_translation(c), Vec2::splat(tile), color);
        }
    }
}

fn quick_save(g: &Game, ui: &mut UiState) {
    match pw_save::save(g, &ui.save_path) {
        Ok(n) => ui.toast(format!("Saved ({n} changed chunks) to {}", ui.save_path.display())),
        Err(e) => ui.toast(format!("Save failed: {e}")),
    }
}

fn quick_load(g: &mut Game, ui: &mut UiState) {
    match pw_save::load(g, &ui.save_path) {
        Ok(w) if w.is_empty() => ui.toast("Loaded"),
        Ok(w) => ui.toast(format!("Loaded with {} warnings: {}", w.len(), w.join("; "))),
        Err(e) => ui.toast(format!("Load failed: {e}")),
    }
}

/// Reload every mod from disk and start a fresh world with this seed.
fn new_world(game: &mut Game, seed: u64, ui: &mut UiState) {
    let (content, scripts, report) = pw_mods::load_all(&game.mods_dir);
    if !report.ok() {
        ui.toast(format!("Mod errors: {}", report.errors.join(" | ")));
        return;
    }
    *game = Game::new(Arc::new(content), scripts, game.ext.clone(), seed, game.mods_dir.clone());
    ui.toast(format!("New world, seed {seed}"));
}

fn tool_button(ui: &mut egui::Ui, state: &mut UiState, tool: Tool, label: &str) {
    let text = match hotkey_label(state, &tool) {
        Some(k) => format!("{label}  [{k}]"),
        None => label.to_string(),
    };
    if ui.selectable_label(state.tool == tool, text).clicked() {
        state.tool = tool;
    }
}

/// Food, rest, warmth and health as coloured bars.
fn needs_bars(ui: &mut egui::Ui, n: &pw_game::Needs) {
    egui::Grid::new("needs").spacing([8.0, 2.0]).show(ui, |ui| {
        for (name, v) in [("Food", n.food), ("Rest", n.rest), ("Warmth", n.warmth), ("Health", n.health)] {
            let color = if v < 0.2 {
                egui::Color32::from_rgb(200, 70, 60)
            } else if v < 0.4 {
                egui::Color32::from_rgb(210, 160, 60)
            } else {
                egui::Color32::from_rgb(80, 150, 90)
            };
            ui.label(egui::RichText::new(name).small());
            ui.add(egui::ProgressBar::new(v).desired_width(140.0).fill(color).text(format!("{:.0}%", v * 100.0)));
            ui.end_row();
        }
    });
    ui.label(egui::RichText::new(format!("Feels like {:.0} C", n.felt)).small().weak());
}

/// A small line chart of a history, scaled to its own maximum.
fn sparkline(ui: &mut egui::Ui, history: &[u32], color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(110.0, 20.0), egui::Sense::hover());
    let max = history.iter().copied().max().unwrap_or(1).max(1) as f32;
    let n = history.len().max(2) as f32;
    let pts: Vec<egui::Pos2> = history
        .iter()
        .enumerate()
        .map(|(i, &v)| egui::pos2(rect.left() + rect.width() * i as f32 / (n - 1.0), rect.bottom() - rect.height() * v as f32 / max))
        .collect();
    if pts.len() > 1 {
        ui.painter().add(egui::Shape::line(pts, egui::Stroke::new(1.5, color)));
    }
}

fn color32(hex: &str) -> egui::Color32 {
    let c = parse_color(hex).unwrap_or([200, 200, 200, 255]);
    egui::Color32::from_rgb(c[0], c[1], c[2])
}

/// Every tribe: how it lives, what it has learned and discovered, and its story.
fn tribes_window(ui: &mut egui::Ui, g: &Game, goto: &mut Option<Vec2>) {
    ui.label(
        egui::RichText::new(
            "Villages that live on their own. Each learns which work pays off, discovers techs, grows when fed and housed, \
             and sends bands off to found daughter tribes that carry its culture on, a little changed.",
        )
        .weak(),
    );
    if g.tribes.list.is_empty() {
        ui.label("No tribes yet. The 'Found a tribe' power (Life) starts one.");
        return;
    }
    let mut list: Vec<&pw_tribe::Tribe> = g.tribes.list.iter().collect();
    list.sort_by_key(|t| (t.gone.is_some(), std::cmp::Reverse(t.members.len())));
    egui::ScrollArea::vertical().max_height(480.0).show(ui, |ui| {
        for (i, t) in list.into_iter().enumerate() {
            let Some(def) = t.def(&g.content) else { continue };
            let color = color32(&t.color);
            let mut title = egui::RichText::new(format!("The {}  ({} people)", t.name, t.members.len())).color(color).strong();
            if t.gone.is_some() {
                title = egui::RichText::new(format!("The {} (gone)", t.name)).weak().strikethrough();
            }
            egui::CollapsingHeader::new(title).id_salt(("tribe", t.id)).default_open(i == 0).show(ui, |ui| {
                ui.horizontal(|ui| {
                    let fd = t.food_days(def, &g.content);
                    ui.label(format!(
                        "{} beds · food for {:.0} days · wood {} · stone {}",
                        t.beds,
                        fd.min(99.0),
                        t.item_total(&g.content, &def.wood),
                        t.item_total(&g.content, &def.stone)
                    ));
                    if t.gone.is_none() && ui.small_button("Go to").clicked() {
                        let (x, y) = t.center.center();
                        *goto = Some(Vec2::new(x, y));
                    }
                });
                let from = t.parent.and_then(|p| g.tribes.get(p)).map(|p| format!("split from the {}, ", p.name)).unwrap_or_default();
                let days = (g.tick.saturating_sub(t.founded)) as f32 / pw_game::DAY_TICKS as f32;
                ui.label(format!(
                    "{} · {from}generation {} · founded {days:.0} days ago · {} born, {} died, {} animals hunted",
                    def.name, t.generation, t.born, t.died, t.hunted
                ));
                if t.gone.is_none() {
                    ui.label(format!("Now: {}. They value {} most.", pw_tribe::FOCUS_NAMES[t.focus as usize].to_lowercase(), t.values().join(" and ")));
                }
                ui.horizontal(|ui| {
                    ui.label("People:");
                    sparkline(ui, &t.history, color);
                });
                ui.label(egui::RichText::new("Culture (learned from what paid off)").small().strong());
                egui::Grid::new(("culture", t.id)).spacing([8.0, 2.0]).show(ui, |ui| {
                    for f in 0..pw_tribe::FOCI {
                        ui.label(egui::RichText::new(pw_tribe::VALUE_NAMES[f]).small());
                        ui.add(egui::ProgressBar::new(t.culture[f] / 3.0).desired_width(120.0).text(format!("{:.2}", t.culture[f])));
                        ui.end_row();
                    }
                });
                let traits: Vec<String> = (0..pw_tribe::TRAITS).map(|k| format!("{} {:.2}", pw_tribe::TRAIT_NAMES[k], t.traits[k])).collect();
                ui.label(egui::RichText::new(format!("Traits: {}", traits.join(", "))).small());
                let known: Vec<&str> = t.known_techs(def).iter().map(|x| x.name.as_str()).collect();
                ui.label(format!("Knows: {}", known.join(", ")));
                if let Some(r) = t.research.as_ref().and_then(|r| def.techs.iter().find(|x| &x.id == r)) {
                    ui.add(egui::ProgressBar::new((t.points / r.points.max(1.0)).min(1.0)).desired_width(220.0).text(format!("Discovering {}", r.name)));
                }
                ui.label(egui::RichText::new("Story").small().strong());
                for line in t.chronicle.iter().rev().take(8) {
                    ui.label(egui::RichText::new(line).small().weak());
                }
            });
        }
    });
}

/// Every species that has lived here: how it's doing and how it has changed.
fn life_window(ui: &mut egui::Ui, g: &Game) {
    let life = &g.life;
    ui.label(egui::RichText::new(format!(
        "{} wild animals. Genes start at 1.00 and drift with every generation; new species split off when a lineage drifts far enough.",
        life.alive()
    ))
    .weak());
    let mut avg: std::collections::HashMap<u32, ([f32; pw_life::GENES], u32)> = Default::default();
    for c in &life.creatures {
        let e = avg.entry(c.species).or_insert(([0.0; pw_life::GENES], 0));
        for k in 0..pw_life::GENES {
            e.0[k] += c.genes[k];
        }
        e.1 += 1;
    }
    let mut list: Vec<&pw_life::Species> = life.species.iter().collect();
    list.sort_by_key(|s| (s.extinct_tick.is_some(), std::cmp::Reverse(s.alive)));
    egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
        egui::Grid::new("life_grid").striped(true).spacing([10.0, 4.0]).show(ui, |ui| {
            for h in ["Species", "Alive", "Trend", "Born", "Died (eaten / starved / old / burnt / froze / hunted)", "Gen", "Speed", "Size", "Cold", "Fertile"] {
                ui.label(egui::RichText::new(h).small().strong());
            }
            ui.end_row();
            for s in list {
                let name = match s.parent.and_then(|p| life.species_of(p)) {
                    Some(p) => format!("{}  ← {}", s.name, p.name),
                    None => s.name.clone(),
                };
                let text = egui::RichText::new(name);
                ui.label(if s.extinct_tick.is_some() { text.weak().strikethrough() } else { text });
                ui.label(s.alive.to_string());
                // Population over the last hours.
                let (rect, _) = ui.allocate_exact_size(egui::vec2(90.0, 18.0), egui::Sense::hover());
                let max = s.history.iter().copied().max().unwrap_or(1).max(1) as f32;
                let n = s.history.len().max(2) as f32;
                let pts: Vec<egui::Pos2> = s
                    .history
                    .iter()
                    .enumerate()
                    .map(|(i, &v)| egui::pos2(rect.left() + rect.width() * i as f32 / (n - 1.0), rect.bottom() - rect.height() * v as f32 / max))
                    .collect();
                if pts.len() > 1 {
                    ui.painter().add(egui::Shape::line(pts, egui::Stroke::new(1.5, egui::Color32::from_rgb(120, 200, 140))));
                }
                ui.label(s.born.to_string());
                let c = s.causes;
                ui.label(format!("{}  ({} / {} / {} / {} / {} / {})", s.died, c[2], c[0], c[1], c[3], c[4], c[5]));
                ui.label(s.max_generation.to_string());
                match avg.get(&s.id) {
                    Some((sum, n)) => {
                        let n = *n as f32;
                        for k in [pw_life::G_SPEED, pw_life::G_SIZE, pw_life::G_COLD, pw_life::G_FERTILITY] {
                            ui.label(format!("{:.2}", sum[k] / n));
                        }
                    }
                    None => {
                        for _ in 0..4 {
                            ui.label("-");
                        }
                    }
                }
                ui.end_row();
            }
        });
    });
}

/// Colonists x jobs. Click a cell to cycle 1 (first) .. 4 (last) .. off.
fn work_grid(ui: &mut egui::Ui, g: &mut Game) {
    ui.label(egui::RichText::new("Click to change: 1 = do first, 4 = do last, blank = never.").weak());
    let content = g.content.clone();
    let jobs: Vec<usize> = content.jobs_by_priority();
    egui::Grid::new("work_grid").striped(true).spacing([6.0, 4.0]).show(ui, |ui| {
        ui.label("");
        for &j in &jobs {
            let jd = content.jobs.by_idx(j);
            // Column headers: short label, full label on hover.
            let short: String = jd.label.split_whitespace().next().unwrap_or("").chars().take(9).collect();
            ui.label(egui::RichText::new(short).small()).on_hover_text(&jd.label);
        }
        ui.end_row();
        for p in &mut g.colony.pawns {
            ui.label(&p.name);
            for &j in &jobs {
                let id = &content.jobs.by_idx(j).id;
                let v = p.work_priority(id);
                let (text, color) = match v {
                    0 => ("  ".to_string(), egui::Color32::GRAY),
                    1 => ("1".into(), egui::Color32::from_rgb(120, 230, 120)),
                    2 => ("2".into(), egui::Color32::from_rgb(200, 220, 120)),
                    3 => ("3".into(), egui::Color32::from_rgb(220, 200, 150)),
                    n => (n.to_string(), egui::Color32::from_rgb(170, 160, 150)),
                };
                let b = egui::Button::new(egui::RichText::new(text).color(color).strong()).min_size(egui::vec2(28.0, 20.0));
                if ui.add(b).clicked() {
                    p.cycle_work(id);
                }
            }
            ui.end_row();
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn ui_panels(
    mut contexts: EguiContexts,
    mut ui_state: ResMut<UiState>,
    mut game: ResMut<GameRes>,
    mut clock: ResMut<SimClock>,
    mut settings: ResMut<RenderSettings>,
    time: Res<Time>,
    mut cam: Query<&mut Transform, With<MainCamera>>,
) -> Result {
    let ctx = contexts.ctx_mut()?.clone();
    let st = ui_state.as_mut();
    let g = &mut game.0;

    // ---- top bar
    egui::Window::new("topbar")
        .title_bar(false)
        .resizable(false)
        .movable(false)
        .anchor(egui::Align2::CENTER_TOP, [0.0, 6.0])
        .show(&ctx, |ui| {
            ui.horizontal(|ui| {
                let (_, h, m) = g.day_time();
                let cal = g.calendar();
                ui.strong(format!("{} {}  {h:02}:{m:02}", g.season_name(), cal.day_of_season))
                    .on_hover_text(format!("Year {}, day {} of {}", cal.year, cal.day_of_season, g.season_name()));
                ui.separator();
                let env = &g.env;
                let weather = env.weather_def(&g.content.climate).map(|w| w.name.clone()).unwrap_or_default();
                // Compass direction the wind blows toward.
                let arrows = ["E", "NE", "N", "NW", "W", "SW", "S", "SE"];
                let a = ((env.wind_dir.rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU * 8.0).round() as usize) % 8;
                ui.label(format!("{:.0}°C {}", env.temp, weather)).on_hover_text(format!(
                    "{}. Wind {:.0} km/h {}. Ground wetness {:.0}%, snow {:.0}%.",
                    pw_game::env::feels(env.temp),
                    env.wind * 60.0,
                    arrows[a],
                    env.wetness * 100.0,
                    env.snow * 100.0
                ));
                ui.label(format!("Wind {:.0} {}", env.wind * 60.0, arrows[a])).on_hover_text("Wind speed (km/h) and the direction it blows toward");
                ui.separator();
                for (s, label) in [(0u8, "⏸"), (1, "▶"), (2, "▶▶"), (3, "▶▶▶")] {
                    if ui.selectable_label(clock.speed == s, label).clicked() {
                        clock.speed = s;
                    }
                }
                ui.separator();
                ui.label(format!("👥 {}", g.colony.pawns.len()));
                let c = g.content.clone();
                for (i, def) in c.items.iter() {
                    let n = g.colony.items.total(i);
                    if n > 0 {
                        ui.label(format!("{} {n}", def.name));
                    }
                }
                ui.separator();
                if ui.button("Save").on_hover_text("F5").clicked() {
                    quick_save(g, st);
                }
                if ui.button("Load").on_hover_text("F9").clicked() {
                    quick_load(g, st);
                }
                ui.separator();
                ui.checkbox(&mut settings.show_overlay, "Roofs (R)").on_hover_text("Show roofs and tree tops");
                ui.checkbox(&mut settings.see_through, "See-through (V)")
                    .on_hover_text("Roofs and tree tops fade around colonists under them and around the mouse");
                if ui.button("?").clicked() {
                    st.show_help = !st.show_help;
                }
            });
        });

    // ---- tools
    egui::Window::new("Tools")
        .resizable(false)
        .movable(false)
        .anchor(egui::Align2::LEFT_TOP, [6.0, 48.0])
        .default_width(190.0)
        .show(&ctx, |ui| {
            egui::ScrollArea::vertical().max_height(ctx.content_rect().height() - 120.0).show(ui, |ui| {
                tool_button(ui, st, Tool::Inspect, "🔍 Inspect (Esc)");
                ui.collapsing("Orders", |ui| {
                    let c = g.content.clone();
                    for (_, j) in c.jobs.iter() {
                        if j.designatable && j.driver != JobDriver::Plant {
                            tool_button(ui, st, Tool::Order(j.id.clone()), &j.label);
                        }
                    }
                    tool_button(ui, st, Tool::Cancel, "✖ Cancel orders");
                });
                ui.collapsing("Zones", |ui| {
                    tool_button(ui, st, Tool::Stockpile, "📦 Stockpile");
                    tool_button(ui, st, Tool::Unzone, "Remove zone");
                });
                ui.collapsing("Build", |ui| {
                    let c = g.content.clone();
                    let mut cats: Vec<String> =
                        c.protos.iter().filter_map(|p| p.build.as_ref().map(|b| b.category.clone())).collect();
                    cats.sort();
                    cats.dedup();
                    for cat in cats {
                        ui.label(egui::RichText::new(&cat).weak());
                        for (i, d) in c.objects.iter() {
                            if let Some(b) = &c.protos[i].build {
                                if b.category == cat {
                                    let cost: Vec<String> =
                                        b.cost.iter().map(|(it, n)| format!("{n} {}", c.items.by_idx(*it).name)).collect();
                                    let r = ui.selectable_label(st.tool == Tool::Build(d.id.clone()), &d.name);
                                    if r.clone().on_hover_text(cost.join(", ")).clicked() {
                                        st.tool = Tool::Build(d.id.clone());
                                        st.last_build = d.id.clone();
                                    }
                                }
                            }
                        }
                    }
                    ui.label(egui::RichText::new("Walls drag as a room outline. B = last pick").weak().small());
                });
                ui.collapsing("Plant", |ui| {
                    let c = g.content.clone();
                    for (i, d) in c.objects.iter() {
                        if c.protos[i].growth.is_some() && c.protos[i].has_tag("plant") {
                            if ui.selectable_label(st.tool == Tool::Plant(d.id.clone()), &d.name).clicked() {
                                st.tool = Tool::Plant(d.id.clone());
                                st.last_plant = d.id.clone();
                            }
                        }
                    }
                });
                ui.collapsing("God powers", |ui| {
                    let c = g.content.clone();
                    let mut cats: Vec<String> = c.powers.iter().map(|(_, p)| p.category.clone()).collect();
                    cats.dedup();
                    cats.sort();
                    cats.dedup();
                    for cat in cats {
                        ui.label(egui::RichText::new(&cat).weak());
                        ui.horizontal_wrapped(|ui| {
                            for (_, p) in c.powers.iter().filter(|(_, p)| p.category == cat) {
                                let on = st.tool == Tool::Power(p.id.clone());
                                if ui.selectable_label(on, &p.name).on_hover_text(&p.description).clicked() {
                                    st.tool = Tool::Power(p.id.clone());
                                    st.power_radius = p.radius;
                                }
                            }
                        });
                    }
                    ui.add(egui::Slider::new(&mut st.power_radius, 1..=60).text("brush"));
                    ui.separator();
                    tool_button(ui, st, Tool::Paint, "🖌 Paint particles");
                    egui::ComboBox::from_id_salt("paint_mat")
                        .selected_text(c.materials.get(&st.paint_mat).map(|m| m.name.as_str()).unwrap_or("?"))
                        .show_ui(ui, |ui| {
                            for (_, m) in c.materials.iter() {
                                ui.selectable_value(&mut st.paint_mat, m.id.clone(), &m.name);
                            }
                        });
                    egui::ComboBox::from_id_salt("paint_layer").selected_text(st.paint_layer.name()).show_ui(ui, |ui| {
                        for l in Layer::ALL {
                            ui.selectable_value(&mut st.paint_layer, l, l.name());
                        }
                    });
                    ui.add(egui::Slider::new(&mut st.paint_radius, 0..=24).text("radius"));
                    ui.separator();
                    egui::ComboBox::from_id_salt("place_def")
                        .selected_text(c.objects.get(&st.place_def).map(|m| m.name.as_str()).unwrap_or("?"))
                        .show_ui(ui, |ui| {
                            for (_, d) in c.objects.iter() {
                                ui.selectable_value(&mut st.place_def, d.id.clone(), &d.name);
                            }
                        });
                    let place = Tool::Place(st.place_def.clone());
                    tool_button(ui, st, place, "🌳 Place object");
                    tool_button(ui, st, Tool::Ignite, "🔥 Start fire");
                    tool_button(ui, st, Tool::SpawnColonist, "🧍 Add colonist");
                });
                ui.separator();
                if ui.button("🎨 Object editor").clicked() {
                    st.show_editor = !st.show_editor;
                }
                if ui.button("🛠 Data editor (F2)").on_hover_text("See and change how anything in the game is defined").clicked() {
                    st.show_data = !st.show_data;
                }
                ui.collapsing("World", |ui| {
                    ui.checkbox(&mut st.live_reload, "Live reload mods").on_hover_text("Pick up changes to mod files while playing");
                    ui.horizontal(|ui| {
                        ui.label("Seed");
                        ui.add(egui::TextEdit::singleline(&mut st.seed_text).desired_width(90.0));
                        if ui.button("🎲").on_hover_text("Random seed").clicked() {
                            st.seed_text = (time.elapsed().as_nanos() as u64 % 1_000_000_000).to_string();
                        }
                    });
                    if ui.button("New world").on_hover_text("Reloads all mods from disk").clicked() {
                        let seed = st.seed_text.trim().parse::<u64>().unwrap_or_else(|_| hash_str(st.seed_text.trim()));
                        new_world(g, seed, st);
                        if let Ok(mut t) = cam.single_mut() {
                            let (x, y) = g.spawn_tile().center();
                            t.translation.x = x;
                            t.translation.y = y;
                        }
                    }
                });
                ui.collapsing("Mods", |ui| {
                    for m in &g.content.mods {
                        ui.label(format!("{} {}", m.name, m.version)).on_hover_text(&m.description);
                    }
                    ui.label(egui::RichText::new("Edit files in assets/mods, then New world").weak().small());
                });
            });
        });

    // ---- info
    egui::Window::new("Info")
        .resizable(false)
        .movable(false)
        .anchor(egui::Align2::RIGHT_TOP, [-6.0, 48.0])
        .default_width(250.0)
        .show(&ctx, |ui| {
            if let Some(cur) = st.cursor {
                for line in g.describe(cur.x.floor() as i32, cur.y.floor() as i32) {
                    ui.label(line);
                }
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.strong("Colonists");
                if ui.small_button("Work priorities (P)").clicked() {
                    st.show_work = !st.show_work;
                }
                if ui.small_button("Life (L)").clicked() {
                    st.show_life = !st.show_life;
                }
                if ui.small_button("Tribes (K)").clicked() {
                    st.show_tribes = !st.show_tribes;
                }
            });
            let mut focus = None;
            for p in &g.colony.pawns {
                ui.horizontal(|ui| {
                    let name = egui::RichText::new(&p.name);
                    let name = if st.selected == Some(p.id) { name.strong().underline() } else { name };
                    if ui.link(name).clicked() {
                        focus = Some((p.id, p.pos));
                    }
                    ui.label(egui::RichText::new(p.activity()).weak());
                    if let Some(w) = p.needs.worst() {
                        ui.label(egui::RichText::new(w).small().color(egui::Color32::from_rgb(230, 120, 90)));
                    }
                });
            }
            if let Some((id, pos)) = focus {
                st.selected = Some(id);
                if let Ok(mut t) = cam.single_mut() {
                    t.translation.x = pos.0;
                    t.translation.y = pos.1;
                }
            }
            if let Some(p) = st.selected.and_then(|id| g.colony.pawns.iter().find(|p| p.id == id)) {
                ui.separator();
                ui.strong(&p.name);
                ui.label(format!("Doing: {}", p.activity()));
                if let Some((item, n)) = p.carrying {
                    ui.label(format!("Carrying: {n} {}", g.content.items.by_idx(item).name));
                }
                if g.content.creatures.by_idx(p.creature).needs.is_some() {
                    needs_bars(ui, &p.needs);
                }
                ui.horizontal(|ui| {
                    let label = if st.follow { "Stop following (F)" } else { "Follow (F)" };
                    if ui.button(label).clicked() {
                        st.follow = !st.follow;
                    }
                    if ui.button("Deselect").clicked() {
                        st.selected = None;
                        st.follow = false;
                    }
                });
            } else if st.selected.is_some() {
                st.selected = None;
                st.follow = false;
            }
            ui.separator();
            ui.label(format!(
                "Orders {}  Blueprints {}  Chunks {} (+{} loading)",
                g.colony.designations.len(),
                g.colony.blueprints.len(),
                g.world.chunks.len(),
                g.pending_chunks()
            ));
            ui.separator();
            for m in g.log.iter().rev().take(6) {
                ui.label(egui::RichText::new(m).small());
            }
        });

    if let Some((msg, t)) = &mut st.toast {
        *t -= time.delta_secs();
        let m = msg.clone();
        if *t <= 0.0 {
            st.toast = None;
        }
        egui::Window::new("toast")
            .title_bar(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -16.0])
            .show(&ctx, |ui| ui.label(m));
    }

    // ---- hover tooltip over the map
    if st.tool == Tool::Inspect && !ctx.is_pointer_over_egui() {
        if let (Some(cur), Some(at)) = (st.cursor, ctx.pointer_hover_pos()) {
            let mut lines: Vec<String> = Vec::new();
            if let Some(p) = pawn_at(g, cur, 6.0).and_then(|id| g.colony.pawns.iter().find(|p| p.id == id)) {
                lines.push(format!("{} - {}", p.name, p.activity()));
                if g.content.creatures.by_idx(p.creature).needs.is_some() {
                    let n = &p.needs;
                    lines.push(format!(
                        "Food {:.0}%  Rest {:.0}%  Warmth {:.0}%  Health {:.0}%",
                        n.food * 100.0,
                        n.rest * 100.0,
                        n.warmth * 100.0,
                        n.health * 100.0
                    ));
                }
            }
            if let Some((t, p)) = g.tribe_pawn_at((cur.x, cur.y), 6.0) {
                lines.extend(g.describe_tribe_pawn(t, p));
                lines.push(format!("Their values: {}", t.values().join(", ")));
            }
            if let Some(c) = g.life.at((cur.x, cur.y), 6.0) {
                let sp = g.life.species_of(c.species).map(|s| s.name.clone()).unwrap_or_default();
                lines.push(format!("{sp} - {}", c.activity()));
                lines.push(format!(
                    "{:.1} days old, generation {}, {} young, {} kills",
                    c.age as f32 / pw_game::DAY_TICKS as f32,
                    c.generation,
                    c.children,
                    c.kills
                ));
                // Its most unusual genes, and what it has learned to favour.
                let mut genes: Vec<(usize, f32)> = (0..pw_life::GENES).map(|k| (k, c.genes[k])).collect();
                genes.sort_by(|a, b| (b.1 - 1.0).abs().total_cmp(&(a.1 - 1.0).abs()));
                let gtxt: Vec<String> = genes.iter().take(3).map(|(k, v)| format!("{} {:.2}", pw_life::GENE_NAMES[*k], v)).collect();
                lines.push(format!("Genes: {}", gtxt.join(", ")));
                let (best, _) = c.brain.iter().enumerate().fold((0, f32::MIN), |b, (i, &w)| if w > b.1 { (i, w) } else { b });
                lines.push(format!("Has learned to favour: {}", pw_life::ACTION_NAMES[best].to_lowercase()));
            }
            // First describe line is the coordinates; skip it.
            lines.extend(g.describe(cur.x.floor() as i32, cur.y.floor() as i32).into_iter().skip(1).take(3));
            if !lines.is_empty() {
                egui::Area::new(egui::Id::new("hover_tip"))
                    .order(egui::Order::Tooltip)
                    .interactable(false)
                    .fixed_pos(at + egui::vec2(18.0, 18.0))
                    .show(&ctx, |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                            for l in &lines {
                                ui.label(egui::RichText::new(l).small());
                            }
                        });
                    });
            }
        }
    }

    if st.show_help {
        egui::Window::new("How to play").collapsible(false).resizable(false).default_pos([240.0, 120.0]).open(&mut st.show_help).show(&ctx, |ui| {
            ui.label("Camera: WASD / arrows, middle-drag, scroll or Q/E to zoom. Shift = faster.");
            ui.label("Time: Space pause, 1 / 2 / 3 speed.");
            ui.label("Pick a tool on the left, then drag over the map.");
            ui.label("  Orders: chop, mine, harvest, break rocks, deconstruct.");
            ui.label("  Zones: stockpiles; colonists haul loose items there.");
            ui.label("  Build: drag walls to draw a room outline. Colonists haul materials and build.");
            ui.label("  Roofs need a wall or rock within 6 tiles, or they can't be built and they fall in.");
            ui.label("  Plant: saplings and sprouts grow into trees and bushes.");
            ui.label("  God powers: paint any material, drop objects, start fires.");
            ui.label("Keys: C chop, M mine, G harvest, X cancel, Z stockpile, B build, T plant.");
            ui.label("Click a colonist to select; F follows them. P: work priorities. L: wild life and evolution. K: tribes.");
            ui.label("Right click or Esc: back to Inspect. R: hide canopies and roofs. V: see-through roofs.");
            ui.label("F5 save, F9 load. Object editor: paint your own trees, houses and props.");
        });
    }

    if st.show_data {
        let mut open = true;
        let mut msg = None;
        egui::Window::new("Data editor").open(&mut open).default_pos([240.0, 70.0]).show(&ctx, |ui| {
            msg = st.data.ui(ui, g);
        });
        if let Some(m) = msg {
            st.toast(m);
        }
        st.show_data = open;
    }

    if st.show_life {
        let mut open = true;
        egui::Window::new("Life").open(&mut open).default_pos([260.0, 140.0]).show(&ctx, |ui| life_window(ui, g));
        st.show_life = open;
    }

    if st.show_tribes {
        let mut open = true;
        let mut goto = None;
        egui::Window::new("Tribes").open(&mut open).default_pos([300.0, 120.0]).default_width(420.0).show(&ctx, |ui| tribes_window(ui, g, &mut goto));
        st.show_tribes = open;
        if goto.is_some() {
            st.goto = goto;
        }
    }

    if st.show_work {
        let mut open = true;
        egui::Window::new("Work priorities").open(&mut open).resizable(false).default_pos([260.0, 120.0]).show(&ctx, |ui| {
            work_grid(ui, g);
        });
        st.show_work = open;
    }

    if st.show_editor {
        let mut open = true;
        let mut place: Option<String> = None;
        egui::Window::new("Object editor").open(&mut open).default_pos([260.0, 80.0]).show(&ctx, |ui| {
            if let Some(id) = st.editor.ui(ui, g) {
                place = Some(id);
            }
        });
        if let Some(id) = place {
            st.place_def = id.clone();
            st.tool = Tool::Place(id);
            st.toast("Saved. Click the map to place it (God powers > Place object).");
        }
        st.show_editor = open;
    }
    Ok(())
}
