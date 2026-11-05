//! People's inner lives: the character window, the chatter feed, speech
//! bubbles over heads, and the settings for hooking a language model into a
//! person's mind.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use pw_game::{Game, MODEL_CONFIG, Who};
use pw_render::{GameRes, MainCamera, SimClock};

use crate::UiState;

const GOOD: egui::Color32 = egui::Color32::from_rgb(110, 190, 110);
const BAD: egui::Color32 = egui::Color32::from_rgb(225, 110, 90);
/// Lines and marks that came from a model.
pub const MODEL: egui::Color32 = egui::Color32::from_rgb(170, 130, 240);

fn signed(v: f32) -> egui::Color32 {
    if v >= 0.0 { GOOD } else { BAD }
}

/// The person nearest a world point, colonist or tribesperson.
pub fn person_at(g: &Game, p: Vec2, reach: f32) -> Option<Who> {
    g.all_pawns()
        .map(|(o, c)| (Vec2::new(c.pos.0, c.pos.1).distance(p), Who { owner: o, id: c.id }))
        .filter(|(d, _)| *d <= reach)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|x| x.1)
}

/// Everything about one person. Returns a place to move the camera to.
pub fn person_window(ui: &mut egui::Ui, g: &mut Game, st: &mut UiState) {
    let Some(w) = st.person else { return };
    let Some(p) = g.person(w).cloned() else {
        ui.label("They are gone.");
        return;
    };
    let m = &p.mind;
    ui.horizontal(|ui| {
        ui.heading(&p.name);
        ui.label(egui::RichText::new(format!("of {}", g.group_name(w.owner))).weak());
    });
    ui.horizontal_wrapped(|ui| {
        let traits = m.persona.traits();
        if traits.is_empty() {
            ui.label(egui::RichText::new("An ordinary sort").small().weak());
        }
        for t in traits {
            ui.label(egui::RichText::new(t).small().background_color(ui.visuals().faint_bg_color));
        }
    })
    .response
    .on_hover_text(format!(
        "Social {:.0}%  Kind {:.0}%  Diligent {:.0}%  Brave {:.0}%  Curious {:.0}%",
        m.persona.social * 100.0,
        m.persona.kind * 100.0,
        m.persona.diligent * 100.0,
        m.persona.brave * 100.0,
        m.persona.curious * 100.0
    ));
    ui.label(format!("Doing: {}", p.activity()));
    if !m.thought.is_empty() {
        ui.label(egui::RichText::new(format!("\u{201C}{}\u{201D}", m.thought)).italics().color(if m.hooked { MODEL } else { ui.visuals().text_color() }));
    }
    if let Some(s) = m.saying(g.tick) {
        ui.label(format!("Saying: \"{s}\""));
    }

    ui.separator();
    let mood_color = if m.mood < 0.25 { BAD } else if m.mood < 0.5 { egui::Color32::from_rgb(210, 160, 60) } else { GOOD };
    ui.add(egui::ProgressBar::new(m.mood).desired_width(220.0).fill(mood_color).text(format!("Mood: {} {:.0}%", m.mood_word(), m.mood * 100.0)));
    let mut feelings = m.moodlets.clone();
    feelings.sort_by(|a, b| b.value.abs().total_cmp(&a.value.abs()));
    for f in feelings.iter().take(6) {
        ui.label(egui::RichText::new(format!("{:+.0}  {}", f.value * 100.0, f.label)).small().color(signed(f.value)));
    }
    ui.add(egui::ProgressBar::new(m.company).desired_width(220.0).text(format!("Company {:.0}%", m.company * 100.0)));
    if g.content.creatures.by_idx(p.creature).needs.is_some() {
        crate::needs_bars(ui, &p.needs);
    }

    ui.separator();
    ui.strong("People they know");
    let mut ops: Vec<(u64, pw_colony::mind::Opinion)> = m.opinions.iter().map(|(k, o)| (*k, o.clone())).collect();
    ops.sort_by(|a, b| b.1.value.total_cmp(&a.1.value));
    if ops.is_empty() {
        ui.label(egui::RichText::new("Nobody yet.").small().weak());
    }
    for (uid, o) in ops.iter().take(8) {
        ui.horizontal(|ui| {
            let name = egui::RichText::new(&o.name).small();
            if ui.link(name).clicked()
                && let Some(other) = g.find_uid(*uid)
            {
                st.person = Some(other);
                if let Some(q) = g.person(other) {
                    st.goto = Some(Vec2::new(q.pos.0, q.pos.1));
                }
            }
            ui.label(egui::RichText::new(format!("{} ({:+.0})", o.word(), o.value)).small().color(signed(o.value)));
        });
    }

    ui.separator();
    ui.collapsing("Memories", |ui| {
        egui::ScrollArea::vertical().max_height(150.0).id_salt("mem").show(ui, |ui| {
            for mem in m.memories.iter().rev() {
                let (day, h, mi) = day_time(mem.tick);
                ui.label(egui::RichText::new(format!("Day {day} {h:02}:{mi:02}  {}", mem.text)).small());
            }
        });
    });

    ui.separator();
    let mut hooked = m.hooked;
    let model = g.social.llm.cfg.model.clone();
    if ui
        .checkbox(&mut hooked, format!("Let a model think for {} ({model})", p.name))
        .on_hover_text("A language model picks what they say and do next. Set it up in Model settings.")
        .changed()
    {
        g.set_hooked(w, hooked);
    }
    if m.hooked {
        let waiting = g.social.llm.busy(m.uid);
        ui.label(egui::RichText::new(if waiting { "Thinking...".to_string() } else { g.social.llm.status.clone() }).small().color(MODEL));
        ui.horizontal(|ui| {
            if ui.small_button("Think now").clicked() {
                g.social.think_now(m.uid);
            }
            if ui.small_button("Show notes sent to the model").on_hover_text("What the model is told about the world").clicked() {
                st.show_notes = !st.show_notes;
            }
        });
        if st.show_notes {
            let notes = g.situation(w);
            egui::ScrollArea::vertical().max_height(180.0).id_salt("notes").show(ui, |ui| {
                ui.label(egui::RichText::new(notes).small().monospace());
            });
        }
    }
    ui.horizontal(|ui| {
        if ui.small_button("Model settings").clicked() {
            st.show_model = !st.show_model;
        }
        let label = if st.follow { "Stop following (F)" } else { "Follow (F)" };
        if ui.small_button(label).clicked() {
            st.follow = !st.follow;
        }
    });
}

fn day_time(tick: u64) -> (u64, u32, u32) {
    let day = tick / pw_game::DAY_TICKS + 1;
    let frac = (tick % pw_game::DAY_TICKS) as f32 / pw_game::DAY_TICKS as f32;
    let minutes = (frac * 24.0 * 60.0) as u32 + 6 * 60;
    (day, (minutes / 60) % 24, minutes % 60)
}

/// Where the model lives and how often it's asked.
pub fn model_window(ui: &mut egui::Ui, g: &mut Game, st: &mut UiState) {
    ui.label("Any OpenAI-style chat endpoint works: Ollama, llama.cpp's server, LM Studio, or a hosted API.");
    ui.label(egui::RichText::new("Ollama: install it, run `ollama pull llama3.2:3b`, keep the default URL. 3B+ models play their part far better than 1B ones.").small().weak());
    let cfg = &mut g.social.llm.cfg;
    egui::Grid::new("model_cfg").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
        ui.label("URL");
        ui.add(egui::TextEdit::singleline(&mut cfg.url).desired_width(300.0));
        ui.end_row();
        ui.label("Model");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut cfg.model).desired_width(220.0));
            if ui.small_button("Find models").on_hover_text("Ask the server what it has").clicked() {
                st.finding = true;
            }
        });
        ui.end_row();
        ui.label("API key");
        ui.add(egui::TextEdit::singleline(&mut cfg.api_key).password(true).desired_width(300.0).hint_text("only for hosted APIs"));
        ui.end_row();
        ui.label("Think every");
        ui.add(egui::Slider::new(&mut cfg.think_every, 3.0..=120.0).suffix(" s"));
        ui.end_row();
        ui.label("Creativity");
        ui.add(egui::Slider::new(&mut cfg.temperature, 0.0..=1.5));
        ui.end_row();
        ui.label("Max reply");
        ui.add(egui::Slider::new(&mut cfg.max_tokens, 40..=600).suffix(" tokens"));
        ui.end_row();
        ui.label("");
        ui.checkbox(&mut cfg.json_mode, "Ask for JSON output").on_hover_text("Helps small models answer in the right shape. Turned off automatically if the server refuses it.");
        ui.end_row();
    });
    if st.finding {
        st.finding = false;
        g.social.llm.find_models();
    }
    let found = g.social.llm.found.lock().ok().and_then(|f| f.clone());
    match found {
        Some(Ok(names)) => {
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new("Pick:").small().weak());
                for n in names {
                    if ui.selectable_label(g.social.llm.cfg.model == n, egui::RichText::new(&n).small()).clicked() {
                        g.social.llm.cfg.model = n;
                    }
                }
            });
        }
        Some(Err(e)) => {
            ui.label(egui::RichText::new(e).small().color(BAD));
        }
        None => {}
    }
    ui.horizontal(|ui| {
        if ui.button("Test").clicked() {
            g.test_model();
        }
        if ui.button("Save").on_hover_text(MODEL_CONFIG).clicked() {
            match g.social.llm.cfg.save(std::path::Path::new(MODEL_CONFIG)) {
                Ok(()) => st.toast(format!("Saved to {MODEL_CONFIG}")),
                Err(e) => st.toast(format!("Couldn't save: {e}")),
            }
        }
    });
    let llm = &g.social.llm;
    ui.label(egui::RichText::new(&llm.status).small().color(if llm.status.starts_with("Error") { BAD } else { MODEL }));
    let hooked: Vec<(Who, String)> = g.all_pawns().filter(|(_, p)| p.mind.hooked).map(|(o, p)| (Who { owner: o, id: p.id }, p.name.clone())).collect();
    ui.label(egui::RichText::new(format!("{} people thinking with it - {} answers, {} errors", hooked.len(), llm.calls, llm.errors)).small().weak());
    ui.horizontal_wrapped(|ui| {
        for (w, name) in hooked {
            if ui.link(egui::RichText::new(name).small().color(MODEL)).clicked() {
                st.person = Some(w);
                st.show_person = true;
            }
        }
    });
}

/// What people have been saying to each other, newest first.
pub fn chatter_window(ui: &mut egui::Ui, g: &Game, st: &mut UiState) {
    if g.social.feed.is_empty() {
        ui.label(egui::RichText::new("Nobody has said anything yet. People chat when they meet with time on their hands.").weak());
        return;
    }
    egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
        for l in g.social.feed.iter().rev() {
            ui.horizontal_wrapped(|ui| {
                let (_, h, m) = day_time(l.tick);
                ui.label(egui::RichText::new(format!("{h:02}:{m:02}")).small().weak());
                let name = egui::RichText::new(&l.name).small().strong();
                let name = if l.model { name.color(MODEL) } else { name };
                if ui.link(name).clicked() {
                    st.person = Some(l.who);
                    st.show_person = true;
                    if let Some(p) = g.person(l.who) {
                        st.goto = Some(Vec2::new(p.pos.0, p.pos.1));
                    }
                }
                ui.label(egui::RichText::new(format!("to {}:", l.to)).small().weak());
                ui.label(egui::RichText::new(&l.text).small());
            });
        }
    });
}

/// Words over people's heads, and a mark on those a model is driving.
pub fn speech_bubbles(
    mut contexts: EguiContexts,
    game: Res<GameRes>,
    clock: Res<SimClock>,
    ui_state: Res<UiState>,
    cam: Query<(&Camera, &GlobalTransform, &Projection), With<MainCamera>>,
) -> Result {
    let ctx = contexts.ctx_mut()?.clone();
    let Ok((camera, gt, proj)) = cam.single() else { return Ok(()) };
    let scale = match proj {
        Projection::Orthographic(o) => o.scale,
        _ => 1.0,
    };
    let g = &game.0;
    let tick = g.tick;
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Background, egui::Id::new("bubbles")));
    let screen = ctx.content_rect();
    let close = scale < 0.6;
    for (owner, p) in g.all_pawns() {
        let w = Who { owner, id: p.id };
        let chosen = ui_state.person == Some(w);
        let pos = clock.draw_pos_of(owner, p);
        let top = g.content.creature_rt[p.creature].sprite.h as f32 * 0.5;
        let head = Vec3::new(pos.x, pos.y + top + 1.0, 0.0);
        let Ok(at) = camera.world_to_viewport(gt, head) else { continue };
        let at = egui::pos2(at.x, at.y);
        if !screen.expand(40.0).contains(at) {
            continue;
        }
        if p.mind.hooked {
            // A small violet diamond: this one thinks with a model.
            let c = at + egui::vec2(0.0, -4.0);
            let r = 3.5;
            painter.add(egui::Shape::convex_polygon(
                vec![c + egui::vec2(0.0, -r), c + egui::vec2(r, 0.0), c + egui::vec2(0.0, r), c + egui::vec2(-r, 0.0)],
                MODEL,
                egui::Stroke::new(1.0, egui::Color32::from_rgb(40, 20, 70)),
            ));
        }
        let Some(text) = p.mind.saying(tick) else { continue };
        // Zoomed far out only the chosen and the model-driven still speak up.
        if !close && !chosen && !p.mind.hooked {
            continue;
        }
        let font = egui::FontId::proportional(if close { 12.0 } else { 11.0 });
        let galley = painter.layout(text.to_string(), font, egui::Color32::from_rgb(30, 28, 36), 170.0);
        let size = galley.size() + egui::vec2(12.0, 8.0);
        let rect = egui::Rect::from_min_size(at + egui::vec2(-size.x / 2.0, -size.y - 14.0), size);
        let edge = if p.mind.hooked { MODEL } else { egui::Color32::from_rgb(70, 64, 80) };
        painter.rect(rect, 5.0, egui::Color32::from_rgba_unmultiplied(250, 247, 238, 235), egui::Stroke::new(1.0, edge), egui::StrokeKind::Inside);
        let tip = rect.center_bottom();
        painter.add(egui::Shape::convex_polygon(
            vec![tip + egui::vec2(-4.0, -1.0), tip + egui::vec2(4.0, -1.0), tip + egui::vec2(0.0, 6.0)],
            egui::Color32::from_rgba_unmultiplied(250, 247, 238, 235),
            egui::Stroke::NONE,
        ));
        painter.galley(rect.min + egui::vec2(6.0, 4.0), galley, egui::Color32::from_rgb(30, 28, 36));
    }
    Ok(())
}
