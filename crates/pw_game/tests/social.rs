//! People talk, make friends, and can be driven by a language model.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;

use pw_game::*;

fn mods_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/mods")
}

fn new_game(seed: u64) -> Game {
    let (content, scripts, report) = pw_mods::load_all(&mods_dir());
    assert!(report.ok(), "mod errors: {:#?}", report.errors);
    let mut g = Game::new(Arc::new(content), scripts, Arc::new(Extensions::default()), seed, mods_dir());
    // Never reach for a real model from tests.
    g.social.llm.cfg = llm::LlmConfig { url: "http://127.0.0.1:9/none".into(), ..Default::default() };
    g
}

#[test]
fn everyone_has_a_personality() {
    let g = new_game(3);
    let uids: Vec<u64> = g.all_pawns().map(|(_, p)| p.mind.uid).collect();
    assert!(uids.iter().all(|&u| u != 0));
    let mut sorted = uids.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), uids.len(), "uids are unique");
    // Not everyone is the same person.
    let first = &g.colony.pawns[0].mind.persona;
    assert!(g.colony.pawns.iter().any(|p| (p.mind.persona.kind - first.kind).abs() > 0.01));
}

#[test]
fn colonists_chat_and_form_opinions() {
    let mut g = new_game(5);
    // Nothing to do, all together: they should talk.
    for p in &mut g.colony.pawns {
        p.mind.company = 0.1;
    }
    let mut lines = 0;
    for _ in 0..(DAY_TICKS / 3) {
        g.step();
        lines = g.social.feed.len();
        if lines >= 4 && g.colony.pawns.iter().any(|p| !p.mind.opinions.is_empty()) {
            break;
        }
    }
    assert!(lines >= 4, "only {lines} lines of chatter");
    assert!(g.colony.pawns.iter().any(|p| !p.mind.opinions.is_empty()), "nobody formed an opinion");
    assert!(g.colony.pawns.iter().any(|p| !p.mind.memories.is_empty()), "nobody remembers a chat");
    // Lonely people who chatted feel less lonely.
    assert!(g.colony.pawns.iter().any(|p| p.mind.company > 0.3));
}

#[test]
fn moods_follow_needs() {
    let mut g = new_game(5);
    let id = g.colony.pawns[0].id;
    let before = g.colony.pawns[0].mind.mood;
    // Exhausted: sleep only slowly brings rest back.
    g.colony.pawns[0].needs.rest = 0.02;
    for _ in 0..200 {
        g.step();
    }
    let p = g.colony.pawns.iter().find(|p| p.id == id).unwrap();
    assert!(p.mind.moodlets.iter().any(|m| m.key == "tired"), "{:?}", p.mind.moodlets);
    assert!(p.mind.mood < before, "an exhausted person is less happy: {} -> {}", before, p.mind.mood);
    assert!(!p.mind.thought.is_empty());
}

#[test]
fn deaths_are_mourned() {
    let mut g = new_game(5);
    let (a, b) = (g.colony.pawns[0].mind.uid, g.colony.pawns[1].name.clone());
    let buid = g.colony.pawns[1].mind.uid;
    g.colony.pawns[0].mind.adjust_opinion(buid, &b, 80.0, 0);
    let bid = g.colony.pawns[1].id;
    g.colony.pawns[1].needs.health = 0.0;
    g.colony.pawns[1].needs.food = 0.0;
    for _ in 0..120 {
        g.step();
        if !g.colony.pawns.iter().any(|p| p.id == bid) {
            break;
        }
    }
    assert!(!g.colony.pawns.iter().any(|p| p.id == bid), "should have died");
    let w = g.find_uid(a).unwrap();
    let p = g.person(w).unwrap();
    assert!(p.mind.moodlets.iter().any(|m| m.key == format!("death:{b}") && m.value < -0.2), "{:?}", p.mind.moodlets);
}

/// A one-shot fake model server answering with `content`.
fn fake_model(content: &'static str, answers: usize) -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for _ in 0..answers {
            let Ok((mut s, _)) = l.accept() else { return };
            let mut buf = vec![0u8; 65536];
            let mut got = Vec::new();
            // Read headers and body (Content-Length).
            loop {
                let n = s.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    break;
                }
                got.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&got);
                if let Some(h) = text.find("\r\n\r\n") {
                    let len = text[..h]
                        .lines()
                        .find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                        .unwrap_or(0);
                    if got.len() >= h + 4 + len {
                        break;
                    }
                }
            }
            let body = serde_json_body(content);
            let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            let _ = s.write_all(resp.as_bytes());
        }
    });
    format!("http://127.0.0.1:{port}/v1/chat/completions")
}

fn serde_json_body(content: &str) -> String {
    let escaped = content.replace('\\', "\\\\").replace('"', "\\\"");
    format!("{{\"choices\":[{{\"message\":{{\"role\":\"assistant\",\"content\":\"{escaped}\"}}}}]}}")
}

#[test]
fn a_hooked_person_says_and_does_what_the_model_says() {
    let mut g = new_game(5);
    g.social.llm.cfg.builtin = false;
    g.social.llm.cfg.everyone = false;
    g.social.llm.cfg.url = fake_model(r#"{"thought": "I could use a walk.", "say": "Off to see what's out there!", "do": "explore"}"#, 4);
    let w = Who { owner: 0, id: g.colony.pawns[0].id };
    g.set_hooked(w, true);
    let mut done = false;
    for _ in 0..600 {
        g.step();
        let p = g.person(w).unwrap();
        if p.mind.thought == "I could use a walk." && p.activity() == "Exploring" {
            done = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let p = g.person(w).unwrap();
    assert!(done, "thought {:?}, doing {:?}, status {}", p.mind.thought, p.activity(), g.social.llm.status);
    assert!(g.social.feed.iter().any(|l| l.model && l.text.contains("what's out there")));
    assert!(p.mind.memories.iter().any(|m| m.text.contains("Decided to explore")));
}

#[test]
fn a_dead_model_doesnt_stall_the_world() {
    let mut g = new_game(5);
    let w = Who { owner: 0, id: g.colony.pawns[0].id };
    g.set_hooked(w, true);
    // The world keeps running while the request fails in the background.
    let t0 = std::time::Instant::now();
    while !g.social.llm.status.starts_with("Error") && t0.elapsed().as_secs() < 20 {
        g.step();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(g.social.llm.status.starts_with("Error"), "{}", g.social.llm.status);
    assert!(g.person(w).unwrap().mind.thought.contains("didn't answer"));
}

/// Watch a real model run a colonist: `PW_LIVE_MODEL=llama3.2:1b cargo test -p pw_game --test social live_model -- --ignored --nocapture`
#[test]
#[ignore]
fn live_model() {
    let Ok(model) = std::env::var("PW_LIVE_MODEL") else { return };
    let mut g = new_game(5);
    g.social.llm.cfg = llm::LlmConfig { model, builtin: false, think_every: 8.0, ..Default::default() };
    for p in &mut g.colony.pawns {
        p.mind.company = 0.05;
    }
    let w = Who { owner: 0, id: g.colony.pawns[0].id };
    g.set_hooked(w, true);
    let t0 = std::time::Instant::now();
    let mut seen = 0;
    let mut last_thought = String::new();
    while t0.elapsed().as_secs() < 90 {
        g.step();
        std::thread::sleep(std::time::Duration::from_millis(8));
        let p = g.person(w).unwrap();
        if p.mind.thought != last_thought {
            last_thought = p.mind.thought.clone();
            println!("[{}] {} thinks: {}  (doing: {})", g.tick, p.name, last_thought, p.activity());
        }
        for l in g.social.feed.iter().skip(seen) {
            println!("[{}] {}{} -> {}: {}", l.tick, if l.model { "*" } else { "" }, l.name, l.to, l.text);
        }
        seen = g.social.feed.len();
    }
    println!("--- notes ---\n{}", g.situation(w));
    println!("status: {}", g.social.llm.status);
}

#[test]
fn spoken_orders_are_heard_and_each_listener_decides() {
    let mut g = new_game(5);
    g.social.llm.cfg.builtin = false;
    g.social.llm.cfg.everyone = false;
    g.social.llm.cfg.url = fake_model(r#"{"thought": "Time to head back.", "say": "Everyone, go home now!", "do": "tell:everyone:go_home"}"#, 2);
    // Everyone within earshot of the first colonist.
    let here = g.colony.pawns[0].pos;
    for (i, p) in g.colony.pawns.iter_mut().enumerate().skip(1) {
        p.pos = (here.0 + 6.0 * i as f32, here.1);
    }
    let w = Who { owner: 0, id: g.colony.pawns[0].id };
    g.set_hooked(w, true);
    let others: Vec<Who> = g.colony.pawns.iter().skip(1).map(|p| Who { owner: 0, id: p.id }).collect();
    let answered = |g: &Game| others.iter().all(|o| g.person(*o).is_some_and(|p| p.mind.memories.iter().any(|m| m.text.contains("asked you to go home"))));
    let mut ok = false;
    for _ in 0..800 {
        g.step();
        if answered(&g) {
            ok = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(ok, "listeners never answered; status {}", g.social.llm.status);
    // Each one said yes or no out loud, and the speaker heard about it.
    for o in &others {
        let p = g.person(*o).unwrap();
        let agreed = p.mind.memories.iter().any(|m| m.text.ends_with("you agreed."));
        assert!(g.social.feed.iter().any(|l| l.who == *o), "{} said nothing", p.name);
        if agreed {
            assert_eq!(p.activity(), "Heading home", "{} agreed but is {}", p.name, p.activity());
        }
    }
    let speaker = g.person(w).unwrap();
    assert!(speaker.mind.memories.iter().any(|m| m.text.contains("agreed to") || m.text.contains("refused to")), "{:?}", speaker.mind.memories);
    // A direct order from the player puts their own will on hold.
    g.player_order(others[0], "rest");
    assert!(g.person(others[0]).unwrap().mind.obey_until > g.tick);
}

#[test]
fn people_know_where_they_are_and_what_is_around() {
    let mut g = new_game(5);
    for _ in 0..300 {
        g.step();
    }
    let w = Who { owner: 0, id: g.colony.pawns[0].id };
    let notes = g.situation(w);
    println!("--- colonist\n{notes}");
    for part in ["Where you are:", "It feels", "Around you:", "Your colony:", "Work waiting"] {
        assert!(notes.contains(part), "missing {part:?} in:\n{notes}");
    }
    if let Some((o, p)) = g.all_pawns().find(|(o, _)| *o > 0) {
        let notes = g.situation(Who { owner: o, id: p.id });
        println!("--- villager\n{notes}");
        assert!(notes.contains("Your village"), "{notes}");
    }
}
