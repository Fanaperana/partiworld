//! Live data editor: every definition in the game (materials, objects,
//! reactions, creatures, powers, climate...) as editable text. Apply changes
//! to the running world at once, or save them into the `user` mod so they
//! stay. New entries are made by changing the id.

use bevy_egui::egui;
use pw_core::*;
use pw_game::Game;
use serde::Serialize;
use serde::de::DeserializeOwned;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Materials,
    Items,
    Objects,
    Biomes,
    Reactions,
    Jobs,
    Creatures,
    Powers,
    Tribes,
    Recipes,
    Climate,
    Worldgen,
}

const KINDS: [Kind; 12] = [
    Kind::Materials,
    Kind::Objects,
    Kind::Reactions,
    Kind::Creatures,
    Kind::Powers,
    Kind::Tribes,
    Kind::Recipes,
    Kind::Climate,
    Kind::Biomes,
    Kind::Items,
    Kind::Jobs,
    Kind::Worldgen,
];

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Materials => "Materials",
            Kind::Items => "Items",
            Kind::Objects => "Objects",
            Kind::Biomes => "Biomes",
            Kind::Reactions => "Reactions",
            Kind::Jobs => "Jobs",
            Kind::Creatures => "Creatures",
            Kind::Powers => "Powers",
            Kind::Tribes => "Tribes",
            Kind::Recipes => "Recipes",
            Kind::Climate => "Climate",
            Kind::Worldgen => "World gen",
        }
    }
    /// Folder in a mod (None = a single file at the mod root).
    fn folder(self) -> Option<&'static str> {
        match self {
            Kind::Materials => Some("materials"),
            Kind::Items => Some("items"),
            Kind::Objects => Some("objects"),
            Kind::Biomes => Some("biomes"),
            Kind::Reactions => Some("reactions"),
            Kind::Jobs => Some("jobs"),
            Kind::Creatures => Some("creatures"),
            Kind::Powers => Some("powers"),
            Kind::Tribes => Some("tribes"),
            Kind::Recipes => Some("recipes"),
            Kind::Climate | Kind::Worldgen => None,
        }
    }
    fn single(self) -> bool {
        self.folder().is_none()
    }
}

pub struct DataEditor {
    kind: Kind,
    filter: String,
    selected: Option<String>,
    text: String,
    status: String,
}

impl Default for DataEditor {
    fn default() -> Self {
        Self { kind: Kind::Materials, filter: String::new(), selected: None, text: String::new(), status: String::new() }
    }
}

fn ids<T: Def>(r: &Registry<T>) -> Vec<String> {
    r.ids()
}

fn text_of<T: Def + Serialize>(r: &Registry<T>, id: &str) -> String {
    r.get(id).map(|d| pw_mods::to_ron(d).unwrap_or_else(|e| format!("// {e}"))).unwrap_or_default()
}

fn put<T: Def + DeserializeOwned>(r: &mut Registry<T>, text: &str) -> Result<String, String> {
    let d: T = pw_mods::parse_ron(text)?;
    let id = d.def_id().to_string();
    r.insert(d, "user");
    Ok(id)
}

impl DataEditor {
    fn list(&self, c: &Content) -> Vec<String> {
        match self.kind {
            Kind::Materials => ids(&c.materials).into_iter().filter(|i| i != NONE_ID).collect(),
            Kind::Items => ids(&c.items),
            Kind::Objects => ids(&c.objects),
            Kind::Biomes => ids(&c.biomes),
            Kind::Reactions => ids(&c.reactions),
            Kind::Jobs => ids(&c.jobs),
            Kind::Creatures => ids(&c.creatures),
            Kind::Powers => ids(&c.powers),
            Kind::Tribes => ids(&c.tribes),
            Kind::Recipes => ids(&c.recipes),
            Kind::Climate => vec!["climate".into()],
            Kind::Worldgen => vec!["worldgen".into()],
        }
    }

    fn load(&mut self, c: &Content, id: &str) {
        self.text = match self.kind {
            Kind::Materials => text_of(&c.materials, id),
            Kind::Items => text_of(&c.items, id),
            Kind::Objects => text_of(&c.objects, id),
            Kind::Biomes => text_of(&c.biomes, id),
            Kind::Reactions => text_of(&c.reactions, id),
            Kind::Jobs => text_of(&c.jobs, id),
            Kind::Creatures => text_of(&c.creatures, id),
            Kind::Powers => text_of(&c.powers, id),
            Kind::Tribes => text_of(&c.tribes, id),
            Kind::Recipes => text_of(&c.recipes, id),
            Kind::Climate => pw_mods::to_ron(&c.climate).unwrap_or_default(),
            Kind::Worldgen => pw_mods::to_ron(&c.worldgen).unwrap_or_default(),
        };
        self.selected = Some(id.to_string());
        self.status.clear();
    }

    /// Parse the text into a copy of the content and link it. Returns (content, id).
    fn build(&self, g: &Game) -> Result<(Content, String), String> {
        let mut c = (*g.content).clone();
        let id = match self.kind {
            Kind::Materials => put(&mut c.materials, &self.text)?,
            Kind::Items => put(&mut c.items, &self.text)?,
            Kind::Objects => put(&mut c.objects, &self.text)?,
            Kind::Biomes => put(&mut c.biomes, &self.text)?,
            Kind::Reactions => put(&mut c.reactions, &self.text)?,
            Kind::Jobs => put(&mut c.jobs, &self.text)?,
            Kind::Creatures => put(&mut c.creatures, &self.text)?,
            Kind::Powers => put(&mut c.powers, &self.text)?,
            Kind::Tribes => put(&mut c.tribes, &self.text)?,
            Kind::Recipes => put(&mut c.recipes, &self.text)?,
            Kind::Climate => {
                c.climate = pw_mods::parse_ron(&self.text)?;
                "climate".into()
            }
            Kind::Worldgen => {
                c.worldgen = pw_mods::parse_ron(&self.text)?;
                "worldgen".into()
            }
        };
        c.link().map_err(|e| e.join("; "))?;
        Ok((c, id))
    }

    /// Draw the editor. Returns a message for the toast, if any.
    pub fn ui(&mut self, ui: &mut egui::Ui, g: &mut Game) -> Option<String> {
        let mut toast = None;
        ui.horizontal_wrapped(|ui| {
            for k in KINDS {
                if ui.selectable_label(self.kind == k, k.name()).clicked() && self.kind != k {
                    self.kind = k;
                    self.selected = None;
                    self.text.clear();
                    if k.single() {
                        let id = self.list(&g.content)[0].clone();
                        self.load(&g.content.clone(), &id);
                    }
                }
            }
        });
        ui.separator();
        ui.horizontal_top(|ui| {
            if !self.kind.single() {
                ui.vertical(|ui| {
                    ui.set_width(190.0);
                    ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("search").desired_width(180.0));
                    egui::ScrollArea::vertical().id_salt("data_list").max_height(420.0).show(ui, |ui| {
                        let content = g.content.clone();
                        let f = self.filter.to_lowercase();
                        for id in self.list(&content) {
                            if !f.is_empty() && !id.to_lowercase().contains(&f) {
                                continue;
                            }
                            if ui.selectable_label(self.selected.as_deref() == Some(&id), &id).clicked() {
                                self.load(&content, &id);
                            }
                        }
                    });
                });
            }
            ui.vertical(|ui| {
                if self.selected.is_none() {
                    ui.label("Pick something on the left to see and change how it's defined.");
                    return;
                }
                ui.label(
                    egui::RichText::new("Edit, then Apply to change the running world. Change the id to make a new one. Save keeps it in the 'user' mod.")
                        .weak()
                        .small(),
                );
                egui::ScrollArea::vertical().id_salt("data_text").max_height(380.0).show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut self.text)
                            .code_editor()
                            .desired_width(520.0)
                            .desired_rows(22),
                    );
                });
                ui.horizontal(|ui| {
                    if ui.button("Apply").on_hover_text("Use this in the running world now").clicked() {
                        match self.build(g).and_then(|(c, id)| g.swap_content(c).map(|_| id)) {
                            Ok(id) => {
                                self.status = format!("Applied {id}.");
                                toast = Some(format!("Applied {id}"));
                                self.selected = Some(id);
                            }
                            Err(e) => self.status = e,
                        }
                    }
                    if ui.button("Save to my mod").on_hover_text("Apply and write to assets/mods/user so it stays").clicked() {
                        match self.build(g) {
                            Ok((c, id)) => match g.swap_content(c) {
                                Ok(()) => {
                                    let file = if self.kind.single() { id.clone() } else { id.replace(':', "_") };
                                    match pw_mods::save_text_to_mod(&g.mods_dir, "user", self.kind.folder(), &file, &self.text) {
                                        Ok(p) => {
                                            self.status = format!("Saved to {}", p.display());
                                            toast = Some(format!("Saved {id}"));
                                        }
                                        Err(e) => self.status = e,
                                    }
                                }
                                Err(e) => self.status = e,
                            },
                            Err(e) => self.status = e,
                        }
                    }
                    if ui.button("Revert").clicked() {
                        if let Some(id) = self.selected.clone() {
                            self.load(&g.content.clone(), &id);
                        }
                    }
                });
                if !self.status.is_empty() {
                    let ok = self.status.starts_with("Applied") || self.status.starts_with("Saved");
                    let color = if ok { egui::Color32::from_rgb(140, 220, 140) } else { egui::Color32::from_rgb(240, 120, 110) };
                    ui.colored_label(color, &self.status);
                }
            });
        });
        toast
    }
}
