//! Player-facing UI: camera, mouse tools, panels and the object editor.

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

pub struct PwUiPlugin;

impl Plugin for PwUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UiState>()
            .add_systems(Startup, init_ui)
            .add_systems(Update, camera_keys.run_if(not(egui_wants_any_keyboard_input)))
            .add_systems(Update, (camera_mouse, world_tools).chain().run_if(not(egui_wants_any_pointer_input)))
            .add_systems(Update, draw_tool_preview)
            .add_systems(EguiPrimaryContextPass, ui_panels);
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
) {
    let Ok((mut t, mut p)) = cam.single_mut() else { return };
    let Projection::Orthographic(o) = p.as_mut() else { return };
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
    let fast = if keys.pressed(KeyCode::ShiftLeft) { 2.5 } else { 1.0 };
    t.translation += (d * 900.0 * o.scale * fast * time.delta_secs()).extend(0.0);
    if keys.pressed(KeyCode::KeyQ) {
        o.scale = (o.scale * (1.0 + 2.0 * time.delta_secs())).min(4.0);
    }
    if keys.pressed(KeyCode::KeyE) {
        o.scale = (o.scale / (1.0 + 2.0 * time.delta_secs())).max(0.04);
    }
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
    if keys.just_pressed(KeyCode::KeyH) || keys.just_pressed(KeyCode::F1) {
        ui.show_help = !ui.show_help;
    }
    if keys.just_pressed(KeyCode::Escape) {
        ui.tool = Tool::Inspect;
        ui.drag_start = None;
    }
    if keys.just_pressed(KeyCode::F5) {
        quick_save(&game.0, &mut ui);
    }
    if keys.just_pressed(KeyCode::F9) {
        quick_load(&mut game.0, &mut ui);
    }
}

fn camera_mouse(
    buttons: Res<ButtonInput<MouseButton>>,
    scroll: Res<AccumulatedMouseScroll>,
    motion: Res<AccumulatedMouseMotion>,
    mut cam: Query<(&mut Transform, &mut Projection), With<MainCamera>>,
) {
    let Ok((mut t, mut p)) = cam.single_mut() else { return };
    let Projection::Orthographic(o) = p.as_mut() else { return };
    if scroll.delta.y != 0.0 {
        let f = if scroll.delta.y > 0.0 { 1.0 / 1.15 } else { 1.15 };
        o.scale = (o.scale * f).clamp(0.04, 4.0);
    }
    if buttons.pressed(MouseButton::Middle) {
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
                        if n == 0 { "Can't build there".into() } else { format!("{n} blueprints placed") }
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
        Tool::SpawnColonist if buttons.just_pressed(MouseButton::Left) => g.spawn_colonist(tile_of(x, y)),
        _ => {}
    }
}

fn draw_tool_preview(ui: Res<UiState>, mut gizmos: Gizmos, game: Res<GameRes>) {
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
    if ui.selectable_label(state.tool == tool, label).clicked() {
        state.tool = tool;
    }
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
                let (d, h, m) = g.day_time();
                ui.strong(format!("Day {d}  {h:02}:{m:02}"));
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
                ui.label("Seed");
                ui.add(egui::TextEdit::singleline(&mut st.seed_text).desired_width(90.0));
                if ui.button("New world").on_hover_text("Reloads all mods from disk").clicked() {
                    let seed = st.seed_text.trim().parse::<u64>().unwrap_or_else(|_| hash_str(st.seed_text.trim()));
                    new_world(g, seed, st);
                    if let Ok(mut t) = cam.single_mut() {
                        let (x, y) = g.spawn_tile().center();
                        t.translation.x = x;
                        t.translation.y = y;
                    }
                }
                if ui.button("🎲").clicked() {
                    st.seed_text = (time.elapsed().as_nanos() as u64 % 1_000_000_000).to_string();
                }
                ui.separator();
                ui.checkbox(&mut settings.show_overlay, "Canopy/roofs (R)");
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
                                    }
                                }
                            }
                        }
                    }
                    ui.label(egui::RichText::new("Walls drag as a room outline").weak().small());
                });
                ui.collapsing("Plant", |ui| {
                    let c = g.content.clone();
                    for (i, d) in c.objects.iter() {
                        if c.protos[i].growth.is_some() && c.protos[i].has_tag("plant") {
                            tool_button(ui, st, Tool::Plant(d.id.clone()), &d.name);
                        }
                    }
                });
                ui.collapsing("God powers", |ui| {
                    let c = g.content.clone();
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
            ui.strong("Colonists");
            let mut focus = None;
            for p in &g.colony.pawns {
                ui.horizontal(|ui| {
                    if ui.link(&p.name).clicked() {
                        focus = Some(p.pos);
                    }
                    ui.label(egui::RichText::new(p.activity()).weak());
                });
            }
            if let (Some(pos), Ok(mut t)) = (focus, cam.single_mut()) {
                t.translation.x = pos.0;
                t.translation.y = pos.1;
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

    if st.show_help {
        egui::Window::new("How to play").collapsible(false).resizable(false).default_pos([240.0, 120.0]).open(&mut st.show_help).show(&ctx, |ui| {
            ui.label("Camera: WASD / arrows, middle-drag, scroll or Q/E to zoom. Shift = faster.");
            ui.label("Time: Space pause, 1 / 2 / 3 speed.");
            ui.label("Pick a tool on the left, then drag over the map.");
            ui.label("  Orders: chop, mine, harvest, break rocks, deconstruct.");
            ui.label("  Zones: stockpiles; colonists haul loose items there.");
            ui.label("  Build: drag walls to draw a room outline. Colonists haul materials and build.");
            ui.label("  Plant: saplings and sprouts grow into trees and bushes.");
            ui.label("  God powers: paint any material, drop objects, start fires.");
            ui.label("Right click or Esc: back to Inspect. R: hide canopies and roofs.");
            ui.label("F5 save, F9 load. Object editor: paint your own trees, houses and props.");
        });
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
