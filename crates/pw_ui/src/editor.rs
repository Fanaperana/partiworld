//! In-game particle object editor. Paint a grid on the three layers, set
//! properties, and save it as a RON file in `assets/mods/user/objects/`.
//! The object is added to the running game at once.

use std::collections::BTreeMap;
use std::sync::Arc;

use bevy_egui::egui;
use pw_core::*;
use pw_game::Game;

const CHARS: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789#@$%&*+=?!";

pub struct Editor {
    pub id: String,
    pub name: String,
    pub tags: String,
    pub w: usize,
    pub h: usize,
    /// [layer][row * w + col], row 0 = top.
    cells: [Vec<MatId>; 3],
    layer: usize,
    mat: MatId,
    brush: usize,
    cell_px: f32,
    anchor_tile: bool,
    buildable: bool,
    category: String,
    cost_item: String,
    cost_n: u32,
    work: u32,
    break_fraction: f32,
    drop_item: String,
    drop_min: u32,
    drop_max: u32,
    load_from: String,
    show_all_layers: bool,
    status: String,
    mat_filter: String,
}

impl Default for Editor {
    fn default() -> Self {
        let (w, h) = (16, 16);
        Self {
            id: "my_object".into(),
            name: "My object".into(),
            tags: String::new(),
            w,
            h,
            cells: std::array::from_fn(|_| vec![NONE_MAT; w * h]),
            layer: 1,
            mat: NONE_MAT,
            brush: 1,
            cell_px: 16.0,
            anchor_tile: false,
            buildable: false,
            category: "Furniture".into(),
            cost_item: "core:wood".into(),
            cost_n: 2,
            work: 120,
            break_fraction: 0.5,
            drop_item: String::new(),
            drop_min: 1,
            drop_max: 2,
            load_from: "core:oak_tree".into(),
            show_all_layers: true,
            status: String::new(),
            mat_filter: String::new(),
        }
    }
}

fn to_egui(c: [u8; 4], dim: bool) -> egui::Color32 {
    let a = if dim { c[3] / 3 } else { c[3] };
    egui::Color32::from_rgba_unmultiplied(c[0], c[1], c[2], a)
}

impl Editor {
    fn resize(&mut self, w: usize, h: usize) {
        let mut next: [Vec<MatId>; 3] = std::array::from_fn(|_| vec![NONE_MAT; w * h]);
        for (l, layer) in next.iter_mut().enumerate() {
            for r in 0..h.min(self.h) {
                for c in 0..w.min(self.w) {
                    layer[r * w + c] = self.cells[l][r * self.w + c];
                }
            }
        }
        self.cells = next;
        self.w = w;
        self.h = h;
    }

    fn load(&mut self, c: &Content, id: &str) {
        let Some(i) = c.objects.idx(id) else { return };
        let def = c.objects.by_idx(i);
        let proto = &c.protos[i];
        let cells = proto.cells(1);
        if cells.is_empty() {
            return;
        }
        let (minx, maxx) = cells
            .iter()
            .fold((i32::MAX, i32::MIN), |a, s| (a.0.min(s.dx), a.1.max(s.dx)));
        let (miny, maxy) = cells
            .iter()
            .fold((i32::MAX, i32::MIN), |a, s| (a.0.min(s.dy), a.1.max(s.dy)));
        let (w, h) = (
            ((maxx - minx + 1) as usize).min(64),
            ((maxy - miny + 1) as usize).min(64),
        );
        self.w = w;
        self.h = h;
        self.cells = std::array::from_fn(|_| vec![NONE_MAT; w * h]);
        for s in &cells {
            let (col, row) = ((s.dx - minx) as usize, (maxy - s.dy) as usize);
            if col < w && row < h {
                self.cells[s.layer.index()][row * w + col] = s.mat;
            }
        }
        let short = id.split_once(':').map(|(_, b)| b).unwrap_or(id);
        self.id = format!("{short}_custom");
        self.name = format!("{} (custom)", def.name);
        self.tags = def.tags.join(", ");
        self.anchor_tile = def.anchor == Anchor::Tile;
        self.break_fraction = def.break_fraction;
        self.buildable = def.build.is_some();
        if let Some(b) = &def.build {
            self.category = b.category.clone();
            self.work = b.work;
            if let Some((it, n)) = b.cost.first() {
                self.cost_item = it.clone();
                self.cost_n = *n;
            }
        }
        if let Some(d) = def.drops.first() {
            self.drop_item = d.item.clone();
            self.drop_min = d.min;
            self.drop_max = d.max;
        } else {
            self.drop_item.clear();
        }
        self.status = format!("Loaded {id} ({w}x{h})");
    }

    fn to_def(&self, c: &Content) -> Result<ObjectDef, String> {
        let id = if self.id.contains(':') {
            self.id.trim().to_string()
        } else {
            format!("user:{}", self.id.trim())
        };
        if id.ends_with(':') {
            return Err("give the object an id".into());
        }
        let mut chars: BTreeMap<MatId, char> = BTreeMap::new();
        let mut pool = CHARS.chars();
        let mut layers = Vec::new();
        for l in Layer::ALL {
            let grid = &self.cells[l.index()];
            if grid.iter().all(|&m| m == NONE_MAT) {
                continue;
            }
            let mut rows = Vec::new();
            for r in 0..self.h {
                let mut s = String::new();
                for col in 0..self.w {
                    let m = grid[r * self.w + col];
                    if m == NONE_MAT {
                        s.push('.');
                    } else {
                        let ch = match chars.get(&m) {
                            Some(&ch) => ch,
                            None => {
                                let ch = pool.next().ok_or("too many different materials")?;
                                chars.insert(m, ch);
                                ch
                            }
                        };
                        s.push(ch);
                    }
                }
                rows.push(s);
            }
            layers.push(PatternLayer { layer: l, rows });
        }
        if layers.is_empty() {
            return Err("paint something first".into());
        }
        let palette = chars
            .iter()
            .map(|(m, ch)| (*ch, c.mat_id_str(*m).to_string()))
            .collect();
        let tags: Vec<String> = self
            .tags
            .split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect();
        // Anything tagged as a roof follows the roof rule: a wall or rock within 6 tiles.
        let needs_support = if tags.iter().any(|t| t == "roof") {
            6
        } else {
            0
        };
        Ok(ObjectDef {
            id,
            name: self.name.clone(),
            tags,
            shape: ShapeDef::Pattern { palette, layers },
            anchor: if self.anchor_tile {
                Anchor::Tile
            } else {
                Anchor::Center
            },
            break_fraction: self.break_fraction,
            drops: if self.drop_item.is_empty() {
                vec![]
            } else {
                vec![DropDef {
                    item: self.drop_item.clone(),
                    min: self.drop_min,
                    max: self.drop_max.max(self.drop_min),
                }]
            },
            build: self.buildable.then(|| BuildDef {
                cost: if self.cost_n > 0 {
                    vec![(self.cost_item.clone(), self.cost_n)]
                } else {
                    vec![]
                },
                work: self.work.max(1),
                category: self.category.clone(),
            }),
            growth: None,
            regrow: None,
            hooks: BTreeMap::new(),
            needs_support,
            spread: None,
        })
    }

    /// Save to disk and register in the running game. Returns the new id.
    fn save(&mut self, g: &mut Game) -> Option<String> {
        let def = match self.to_def(&g.content) {
            Ok(d) => d,
            Err(e) => {
                self.status = e;
                return None;
            }
        };
        let path = match pw_mods::save_object_to_mod(&g.mods_dir, "user", &def) {
            Ok(p) => p,
            Err(e) => {
                self.status = format!("save failed: {e}");
                return None;
            }
        };
        let mut c = (*g.content).clone();
        let id = def.id.clone();
        c.objects.insert(def, "user");
        if let Err(e) = c.link() {
            self.status = format!("invalid: {}", e.join("; "));
            return None;
        }
        g.set_content(Arc::new(c));
        self.status = format!("Saved {}", path.display());
        Some(id)
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, g: &mut Game) -> Option<String> {
        let content = g.content.clone();
        let mut placed = None;
        ui.horizontal(|ui| {
            ui.label("Start from");
            egui::ComboBox::from_id_salt("ed_load")
                .selected_text(
                    content
                        .objects
                        .get(&self.load_from)
                        .map(|d| d.name.as_str())
                        .unwrap_or("?"),
                )
                .show_ui(ui, |ui| {
                    for (_, d) in content.objects.iter() {
                        ui.selectable_value(&mut self.load_from, d.id.clone(), &d.name);
                    }
                });
            if ui.button("Load").clicked() {
                let id = self.load_from.clone();
                self.load(&content, &id);
            }
            if ui.button("New").clicked() {
                *self = Editor {
                    cell_px: self.cell_px,
                    ..Editor::default()
                };
            }
        });
        ui.horizontal(|ui| {
            let (mut w, mut h) = (self.w, self.h);
            ui.add(egui::DragValue::new(&mut w).range(1..=64).prefix("w "));
            ui.add(egui::DragValue::new(&mut h).range(1..=64).prefix("h "));
            if (w, h) != (self.w, self.h) {
                self.resize(w, h);
            }
            ui.add(egui::Slider::new(&mut self.cell_px, 4.0..=28.0).text("zoom"));
            ui.checkbox(&mut self.show_all_layers, "show all layers");
        });
        ui.horizontal(|ui| {
            for l in Layer::ALL {
                ui.selectable_value(&mut self.layer, l.index(), l.name());
            }
            ui.separator();
            ui.label("brush");
            for b in [1usize, 2, 3, 5] {
                ui.selectable_value(&mut self.brush, b, b.to_string());
            }
            if ui.button("Fill layer").clicked() {
                self.cells[self.layer]
                    .iter_mut()
                    .for_each(|m| *m = self.mat);
            }
            if ui.button("Clear layer").clicked() {
                self.cells[self.layer]
                    .iter_mut()
                    .for_each(|m| *m = NONE_MAT);
            }
        });

        ui.horizontal_top(|ui| {
            // Canvas.
            let size = egui::vec2(self.w as f32 * self.cell_px, self.h as f32 * self.cell_px);
            let (resp, painter) = ui.allocate_painter(size, egui::Sense::click_and_drag());
            let origin = resp.rect.min;
            painter.rect_filled(resp.rect, 0.0, egui::Color32::from_gray(28));
            for r in 0..self.h {
                for col in 0..self.w {
                    let i = r * self.w + col;
                    let rect = egui::Rect::from_min_size(
                        origin + egui::vec2(col as f32 * self.cell_px, r as f32 * self.cell_px),
                        egui::vec2(self.cell_px, self.cell_px),
                    );
                    if (r + col) % 2 == 0 {
                        painter.rect_filled(rect, 0.0, egui::Color32::from_gray(34));
                    }
                    for l in 0..3 {
                        let m = self.cells[l][i];
                        if m == NONE_MAT || (!self.show_all_layers && l != self.layer) {
                            continue;
                        }
                        let dim = self.show_all_layers && l > self.layer;
                        painter.rect_filled(rect, 0.0, to_egui(content.mat_rt(m).color, dim));
                    }
                }
            }
            if self.cell_px >= 8.0 {
                let stroke = egui::Stroke::new(1.0, egui::Color32::from_black_alpha(60));
                for col in 0..=self.w {
                    let x = origin.x + col as f32 * self.cell_px;
                    painter.line_segment(
                        [egui::pos2(x, origin.y), egui::pos2(x, origin.y + size.y)],
                        stroke,
                    );
                }
                for r in 0..=self.h {
                    let y = origin.y + r as f32 * self.cell_px;
                    painter.line_segment(
                        [egui::pos2(origin.x, y), egui::pos2(origin.x + size.x, y)],
                        stroke,
                    );
                }
                // Tile grid (8x8) in a brighter line.
                let tile = egui::Stroke::new(1.0, egui::Color32::from_white_alpha(40));
                for col in (0..=self.w).step_by(TILE as usize) {
                    let x = origin.x + col as f32 * self.cell_px;
                    painter.line_segment(
                        [egui::pos2(x, origin.y), egui::pos2(x, origin.y + size.y)],
                        tile,
                    );
                }
                for r in (0..=self.h).step_by(TILE as usize) {
                    let y = origin.y + r as f32 * self.cell_px;
                    painter.line_segment(
                        [egui::pos2(origin.x, y), egui::pos2(origin.x + size.x, y)],
                        tile,
                    );
                }
            }
            if let Some(pos) = resp.interact_pointer_pos() {
                let erase = ui.input(|i| i.pointer.secondary_down());
                if resp.dragged() || resp.clicked() || resp.secondary_clicked() || erase {
                    let col = ((pos.x - origin.x) / self.cell_px).floor() as i32;
                    let row = ((pos.y - origin.y) / self.cell_px).floor() as i32;
                    let b = self.brush as i32;
                    for dy in 0..b {
                        for dx in 0..b {
                            let (c, r) = (col + dx - b / 2, row + dy - b / 2);
                            if c >= 0 && r >= 0 && (c as usize) < self.w && (r as usize) < self.h {
                                let m = if erase { NONE_MAT } else { self.mat };
                                self.cells[self.layer][r as usize * self.w + c as usize] = m;
                            }
                        }
                    }
                }
            }

            // Palette.
            ui.vertical(|ui| {
                ui.label("Material (right-drag erases)");
                ui.add(
                    egui::TextEdit::singleline(&mut self.mat_filter)
                        .hint_text("filter")
                        .desired_width(150.0),
                );
                egui::ScrollArea::vertical()
                    .id_salt("ed_pal")
                    .max_height(260.0)
                    .show(ui, |ui| {
                        for (i, m) in content.materials.iter() {
                            let f = self.mat_filter.to_lowercase();
                            if !f.is_empty()
                                && !m.name.to_lowercase().contains(&f)
                                && !m.id.contains(&f)
                            {
                                continue;
                            }
                            ui.horizontal(|ui| {
                                let (r, p) = ui
                                    .allocate_painter(egui::vec2(14.0, 14.0), egui::Sense::hover());
                                p.rect_filled(r.rect, 2.0, to_egui(content.mats[i].color, false));
                                let label = if i == 0 { "Eraser" } else { m.name.as_str() };
                                ui.selectable_value(&mut self.mat, i as MatId, label)
                                    .on_hover_text(&m.id);
                            });
                        }
                    });
            });
        });

        ui.separator();
        egui::Grid::new("ed_props").num_columns(2).show(ui, |ui| {
            ui.label("Id");
            ui.text_edit_singleline(&mut self.id);
            ui.end_row();
            ui.label("Name");
            ui.text_edit_singleline(&mut self.name);
            ui.end_row();
            ui.label("Tags");
            ui.text_edit_singleline(&mut self.tags)
                .on_hover_text("comma separated, e.g. tree, flammable, wall, bush, rock");
            ui.end_row();
            ui.label("Anchor");
            ui.checkbox(&mut self.anchor_tile, "snap to tile (buildings)");
            ui.end_row();
            ui.label("Breaks at");
            ui.add(egui::Slider::new(&mut self.break_fraction, 0.05..=1.0).text("lost"));
            ui.end_row();
            ui.label("Drops");
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("ed_drop")
                    .selected_text(
                        content
                            .items
                            .get(&self.drop_item)
                            .map(|d| d.name.as_str())
                            .unwrap_or("nothing"),
                    )
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.drop_item, String::new(), "nothing");
                        for (_, d) in content.items.iter() {
                            ui.selectable_value(&mut self.drop_item, d.id.clone(), &d.name);
                        }
                    });
                ui.add(egui::DragValue::new(&mut self.drop_min).range(0..=99));
                ui.label("to");
                ui.add(egui::DragValue::new(&mut self.drop_max).range(0..=99));
            });
            ui.end_row();
            ui.label("Buildable");
            ui.checkbox(&mut self.buildable, "colonists can build it");
            ui.end_row();
            if self.buildable {
                ui.label("Cost");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut self.cost_n).range(0..=99));
                    egui::ComboBox::from_id_salt("ed_cost")
                        .selected_text(
                            content
                                .items
                                .get(&self.cost_item)
                                .map(|d| d.name.as_str())
                                .unwrap_or("?"),
                        )
                        .show_ui(ui, |ui| {
                            for (_, d) in content.items.iter() {
                                ui.selectable_value(&mut self.cost_item, d.id.clone(), &d.name);
                            }
                        });
                    ui.add(
                        egui::DragValue::new(&mut self.work)
                            .range(1..=5000)
                            .prefix("work "),
                    );
                });
                ui.end_row();
                ui.label("Category");
                ui.text_edit_singleline(&mut self.category);
                ui.end_row();
            }
        });
        ui.horizontal(|ui| {
            if ui.button("💾 Save & use").clicked() {
                placed = self.save(g);
            }
            ui.label(egui::RichText::new(&self.status).weak());
        });
        placed
    }
}
