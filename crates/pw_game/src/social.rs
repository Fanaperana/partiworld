//! People living together. Every person (colonist or tribesperson) has a
//! mood made of passing feelings, gets lonely, chats with whoever is near,
//! makes friends and rivals, mourns the dead, and sometimes breaks down.
//! A person can be "hooked" to a language model, which then chooses what
//! they say and do; everyone else talks from templates.

use std::collections::{HashMap, VecDeque};

use pw_colony::{Intent, Order, Pawn, Persona, mind::relation_word};
use pw_core::*;

use crate::llm::{Llm, LlmConfig, Purpose, Reply};
use crate::{DAY_TICKS, Game, TPS};

/// Ticks between social updates.
pub const SOCIAL_EVERY: u64 = 10;
/// Ticks between one person's mood updates.
const MOOD_EVERY: u64 = 60;
/// Ticks one line of a chat stays up before the reply.
const LINE_TICKS: u64 = 75;
/// How close two people stand to chat (particles).
const CHAT_RANGE: f32 = TILE as f32 * 2.6;
/// How far words carry.
const HEAR_RANGE: f32 = TILE as f32 * 7.0;
/// "Nearby" for a model's notes, in tiles.
const NEAR_TILES: i32 = 16;
/// Longest wait for a model's line in a chat before using a stock line.
const MODEL_WAIT: u64 = 30 * 20;
/// Answer shapes shown to the model, a different one per person so a small
/// model copying the example doesn't make everyone say the same thing.
const EXAMPLES: [&str; 5] = [
    r#"{"thought": "The wall won't build itself.", "say": "", "do": "work"}"#,
    r#"{"thought": "I wonder what Ada is doing.", "say": "Ada! Wait up!", "do": "talk_to:Ada"}"#,
    r#"{"thought": "Too cold out here.", "say": "Who lit the fire?", "do": "warm_up"}"#,
    r#"{"thought": "We need more wood before dark.", "say": "Bram, fetch some wood, would you?", "do": "tell:Bram:work"}"#,
    r#"{"thought": "My feet hurt.", "say": "I'm sitting down for a bit.", "do": "rest"}"#,
];
/// Most lines kept in the world's chatter feed.
const FEED: usize = 60;
const HOUR: u64 = DAY_TICKS / 24;

/// A person anywhere in the world: owner 0 is the player's colony, else a tribe id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Who {
    pub owner: u32,
    pub id: u32,
}

#[derive(Clone, Debug)]
pub struct Convo {
    pub id: u64,
    pub a: Who,
    pub b: Who,
    pub turn: usize,
    /// Stock lines, a first then alternating; a model may replace a hooked speaker's.
    pub script: Vec<String>,
    pub said: Vec<(Who, String)>,
    pub next_at: u64,
    pub topic: &'static str,
    /// Opinion change from what was said (agreeing, insults...).
    pub tone: f32,
    pub waiting_since: Option<u64>,
    /// A model's line for the next turn.
    pub ready: Option<String>,
}

/// One line in the world's chatter feed.
#[derive(Clone, Debug)]
pub struct FeedLine {
    pub tick: u64,
    pub who: Who,
    pub name: String,
    pub to: String,
    pub text: String,
    /// Came from a model.
    pub model: bool,
}

pub struct Social {
    pub convos: Vec<Convo>,
    next_convo: u64,
    pub llm: Llm,
    /// When each hooked person (uid) next asks the model what to do.
    next_think: HashMap<u64, u64>,
    pub feed: VecDeque<FeedLine>,
    /// Its own dice, so chatter doesn't change how the rest of the world rolls.
    pub rng: Rng,
}

impl Default for Social {
    fn default() -> Self {
        Self::new(LlmConfig::default())
    }
}

impl Social {
    pub fn new(cfg: LlmConfig) -> Self {
        Self { convos: Vec::new(), next_convo: 1, llm: Llm::new(cfg), next_think: HashMap::new(), feed: VecDeque::new(), rng: Rng::new(0x50C1A1) }
    }
    pub fn convo_of(&self, w: Who) -> Option<&Convo> {
        self.convos.iter().find(|c| c.a == w || c.b == w)
    }
    /// Ask a hooked person's model again right away.
    pub fn think_now(&mut self, uid: u64) {
        self.next_think.insert(uid, 0);
    }
    fn push_feed(&mut self, l: FeedLine) {
        self.feed.push_back(l);
        while self.feed.len() > FEED {
            self.feed.pop_front();
        }
    }
}

/// What one person looks like to the social system this tick.
#[derive(Clone)]
struct Snap {
    who: Who,
    pos: (f32, f32),
    asleep: bool,
    busy: bool,
    leisure: bool,
    persona: Persona,
    company: f32,
    chat_after: u64,
}

fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

fn pick<'a>(rng: &mut Rng, v: &[&'a str]) -> &'a str {
    v[rng.below(v.len())]
}

/// "Chopping oak tree" -> "chopping oak trees".
fn work_phrase(label: &str) -> Option<String> {
    let l = label.to_lowercase();
    let skip = ["idle", "wandering", "chatting", "sleeping", "eating", "heading", "warming", "resting", "going", "looking", "fleeing", "collapsed", "upset"];
    if l.is_empty() || skip.iter().any(|s| l.starts_with(s)) {
        return None;
    }
    Some(l)
}

impl Game {
    // ------------------------------------------------------------ finding people

    pub fn person(&self, w: Who) -> Option<&Pawn> {
        if w.owner == 0 {
            self.colony.pawns.iter().find(|p| p.id == w.id)
        } else {
            self.tribes.list.iter().find(|t| t.id == w.owner && t.gone.is_none())?.colony.pawns.iter().find(|p| p.id == w.id)
        }
    }

    pub fn person_mut(&mut self, w: Who) -> Option<&mut Pawn> {
        if w.owner == 0 {
            self.colony.pawns.iter_mut().find(|p| p.id == w.id)
        } else {
            self.tribes.list.iter_mut().find(|t| t.id == w.owner && t.gone.is_none())?.colony.pawns.iter_mut().find(|p| p.id == w.id)
        }
    }

    pub fn people(&self) -> Vec<Who> {
        self.all_pawns().map(|(owner, p)| Who { owner, id: p.id }).collect()
    }

    pub fn find_uid(&self, uid: u64) -> Option<Who> {
        self.all_pawns().find(|(_, p)| p.mind.uid == uid).map(|(owner, p)| Who { owner, id: p.id })
    }

    /// "the colony" or "the Rokimi".
    pub fn group_name(&self, owner: u32) -> String {
        if owner == 0 {
            "the colony".into()
        } else {
            self.tribes.get(owner).map(|t| format!("the {}", t.name)).unwrap_or_else(|| "a lost tribe".into())
        }
    }

    fn home_of(&self, owner: u32) -> TilePos {
        if owner == 0 { self.home } else { self.tribes.get(owner).map(|t| t.center).unwrap_or(self.home) }
    }

    /// Tell one person what to do.
    pub fn order_person(&mut self, w: Who, order: Order) -> bool {
        let env = self.needs_env();
        let content = self.content.clone();
        let colony = if w.owner == 0 {
            &mut self.colony
        } else {
            match self.tribes.list.iter_mut().find(|t| t.id == w.owner) {
                Some(t) => &mut t.colony,
                None => return false,
            }
        };
        colony.order(&mut self.world, &content, &mut self.rng, &env, w.id, order)
    }

    /// Turn the model on (or off) for everyone at once.
    pub fn set_everyone(&mut self, on: bool) {
        self.social.llm.cfg.everyone = on;
        for w in self.people() {
            if let Some(p) = self.person_mut(w) {
                p.mind.hooked = on;
                p.mind.model_set = true;
            }
        }
        self.message(if on { format!("Everyone now thinks with {}.", self.social.llm.cfg.label()) } else { "Nobody thinks with the model now.".to_string() });
    }

    /// A direct order from the player: obeyed, and the person's own will waits
    /// a couple of hours (or until they're told otherwise).
    pub fn player_order(&mut self, w: Who, act: &str) {
        let tick = self.tick;
        self.apply_act(w, act, false);
        if let Some(p) = self.person_mut(w) {
            p.mind.obey_until = tick + HOUR * 2;
            p.mind.request = None;
            p.mind.remember(tick, format!("You were told to {}.", act.replace('_', " ")));
        }
    }

    /// Hook a person's mind to the model (or unhook it).
    pub fn set_hooked(&mut self, w: Who, on: bool) {
        let tick = self.tick;
        let Some(p) = self.person_mut(w) else { return };
        p.mind.hooked = on;
        p.mind.model_set = true;
        let uid = p.mind.uid;
        let name = p.name.clone();
        p.mind.remember(tick, if on { "Something new stirs in your mind." } else { "Your mind feels like your own again." });
        if on {
            self.social.think_now(uid);
            self.message(format!("{name} is now thinking with {}.", self.social.llm.cfg.label()));
        }
    }

    fn snaps(&self) -> Vec<Snap> {
        self.all_pawns()
            .map(|(owner, p)| Snap {
                who: Who { owner, id: p.id },
                pos: p.pos,
                asleep: p.sleeping(),
                busy: p.tending_need() || p.drafted,
                leisure: p.at_leisure(),
                persona: p.mind.persona.clone(),
                company: p.mind.company,
                chat_after: p.mind.chat_after,
            })
            .collect()
    }

    // ------------------------------------------------------------ the tick

    pub(crate) fn social_tick(&mut self) {
        for r in self.social.llm.poll() {
            self.apply_reply(r);
        }
        if !self.tick.is_multiple_of(SOCIAL_EVERY) {
            return;
        }
        self.give_minds();
        let slot = (self.tick / SOCIAL_EVERY) % (MOOD_EVERY / SOCIAL_EVERY);
        for w in self.people() {
            if (w.id as u64 + w.owner as u64) % (MOOD_EVERY / SOCIAL_EVERY) == slot {
                self.update_mind(w);
            }
        }
        self.advance_convos();
        self.start_convos();
        self.pursue_intents();
        self.hooked_thinking();
    }

    /// People from old saves have no mind yet.
    fn give_minds(&mut self) {
        let missing: Vec<Who> = self.all_pawns().filter(|(_, p)| p.mind.uid == 0).map(|(o, p)| Who { owner: o, id: p.id }).collect();
        for w in missing {
            let m = pw_colony::Mind::new(&mut self.social.rng);
            if let Some(p) = self.person_mut(w) {
                p.mind = m;
            }
        }
        // Everyone thinks for themselves with the model, unless unticked.
        if self.social.llm.cfg.everyone && self.social.llm.cfg.usable() {
            let new: Vec<Who> = self.all_pawns().filter(|(_, p)| !p.mind.model_set).map(|(o, p)| Who { owner: o, id: p.id }).collect();
            for w in new {
                if let Some(p) = self.person_mut(w) {
                    p.mind.hooked = true;
                    p.mind.model_set = true;
                }
            }
        }
    }

    // ------------------------------------------------------------ moods

    fn update_mind(&mut self, w: Who) {
        let tick = self.tick;
        let mut rng = Rng::new(self.social.rng.next_u64());
        let content = self.content.clone();
        let night = self.light().dark > 0.62;
        let wet = if self.env.snowing() {
            Some("Out in the snow")
        } else if self.env.raining() {
            Some("Out in the rain")
        } else {
            None
        };
        let outdoor = self.env.temp;
        let Some(p) = self.person(w) else { return };
        let has_needs = content.creatures.by_idx(p.creature).needs.is_some();
        let roof = pw_colony::roofed(&self.world, &content, p.tile());
        let label = p.activity().to_string();
        let asleep = p.sleeping();
        let n = p.needs.clone();
        let leisure = p.at_leisure();
        let busy = p.tending_need() || p.drafted;
        let soon = tick + MOOD_EVERY + SOCIAL_EVERY;
        let p = self.person_mut(w).unwrap();
        let name = p.name.clone();
        let m = &mut p.mind;
        m.moodlets.retain(|x| x.until > tick);
        let dt = MOOD_EVERY as f32 / DAY_TICKS as f32;
        if !asleep {
            m.company = (m.company - dt * (0.5 + m.persona.social)).max(0.0);
        }
        if has_needs {
            if n.food <= 0.0 {
                m.feel("hungry", "Starving", -0.35, soon);
            } else if n.food < 0.3 {
                m.feel("hungry", "Hungry", -0.12, soon);
            }
            if n.warmth < 0.4 {
                m.feel("cold", "Freezing", if n.warmth <= 0.0 { -0.3 } else { -0.12 }, soon);
            }
            if n.rest < 0.25 {
                m.feel("tired", "Exhausted", -0.1, soon);
            }
            if n.health < 0.5 {
                m.feel("hurt", "In pain", -0.15, soon);
            }
            if n.felt > 16.0 && outdoor < 8.0 && !asleep {
                m.feel("cozy", "Warm by the fire", 0.06, soon);
            }
        }
        if let Some(w) = wet
            && !roof
            && !asleep
        {
            m.feel("wet", w, -0.06, soon);
        }
        if label.starts_with("Eating") {
            m.feel("meal", "Had a meal", 0.08, tick + DAY_TICKS / 3);
        }
        if asleep && label == "Sleeping in a bed" {
            m.feel("slept", "Slept in a bed", 0.06, tick + DAY_TICKS / 2);
        } else if asleep && label != "Sleeping in a bed" {
            m.feel("slept", "Slept on the ground", -0.07, tick + DAY_TICKS / 2);
        }
        let social = m.persona.social;
        if m.company < 0.25 {
            m.feel("lonely", "Lonely", -(0.04 + 0.1 * social), soon);
        } else if m.company > 0.8 {
            m.feel("company", "Good company", 0.05, soon);
        }
        let target = (0.55 + m.moodlets.iter().map(|x| x.value).sum::<f32>()).clamp(0.0, 1.0);
        m.mood += (target - m.mood) * 0.35;
        if !m.hooked || m.thought.is_empty() {
            m.thought = inner_voice(m, &n, has_needs, &label, night, &mut rng);
        }

        // ---- acting on feelings (hooked people leave this to the model)
        if m.hooked || busy || asleep {
            return;
        }
        let (mood, company, curious) = (m.mood, m.company, m.persona.curious);
        let broke = m.moodlets.iter().any(|x| x.key == "break");
        if mood < 0.12 && !broke && rng.chance(0.2) {
            let to = self.person(w).unwrap().tile().offset(rng.range_i32(-10, 10), rng.range_i32(-10, 10));
            let cry = pick(&mut rng, &["I need to be alone.", "Leave me be!", "I can't do this any more."]);
            if self.order_person(w, Order::Walk { to, label: "Wandering off, upset".into() }) {
                let p = self.person_mut(w).unwrap();
                p.mind.feel("break", "Let it all out", 0.12, tick + DAY_TICKS);
                p.mind.remember(tick, "Couldn't take it any more and walked off alone.");
                p.mind.speak(cry, tick + LINE_TICKS * 2);
                if w.owner == 0 {
                    self.message(format!("{name} is having a hard time and wandered off."));
                }
            }
            return;
        }
        // Lonely and free: go and find someone, a friend if possible.
        if leisure && company < 0.35 && social > 0.35 && rng.chance(0.35) {
            let me = self.person(w).unwrap();
            let (here, mine) = (me.pos, me.mind.clone());
            let best = self
                .all_pawns()
                .filter(|(o, p)| !(*o == w.owner && p.id == w.id) && !p.sleeping())
                .map(|(o, p)| {
                    let d = dist(here, p.pos) / TILE as f32;
                    let liking = mine.opinion_of(p.mind.uid);
                    // Friends are worth a longer walk; strangers only from home.
                    (Who { owner: o, id: p.id }, p.mind.uid, p.name.clone(), p.tile(), d - liking * 0.3 + if o == w.owner { 0.0 } else { 15.0 }, d)
                })
                .filter(|x| x.5 > 2.0 && x.5 < 30.0 && mine.opinion_of(x.1) > -20.0)
                .min_by(|a, b| a.4.total_cmp(&b.4));
            if let Some((_, uid, other, at, _, _)) = best
                && self.order_person(w, Order::Walk { to: at, label: format!("Going to see {other}") })
            {
                let p = self.person_mut(w).unwrap();
                p.mind.intent = Some((Intent::Talk(uid), tick + HOUR * 2));
            }
        } else if leisure && curious > 0.7 && rng.chance(0.04) {
            let to = self.person(w).unwrap().tile().offset(rng.range_i32(-18, 18), rng.range_i32(-18, 18));
            self.order_person(w, Order::Walk { to, label: "Exploring".into() });
        }
    }

    /// A death touches everyone who knew them.
    pub(crate) fn mourn(&mut self, name: &str, owner: u32) {
        let tick = self.tick;
        for w in self.people() {
            let Some(p) = self.person_mut(w) else { continue };
            let m = &mut p.mind;
            let known = m.opinions.iter().find(|(_, o)| o.name == name).map(|(k, o)| (*k, o.value));
            match known {
                Some((k, v)) if v > -25.0 => {
                    let hurt = 0.08 + 0.3 * v.max(0.0) / 100.0;
                    m.feel(&format!("death:{name}"), format!("{name} died"), -hurt, tick + DAY_TICKS * 2);
                    m.remember(tick, format!("{name} died. {}", if v >= 25.0 { "I miss them." } else { "Hard to believe." }));
                    m.opinions.remove(&k);
                }
                Some((k, _)) => {
                    m.feel(&format!("death:{name}"), format!("{name} is gone"), 0.03, tick + DAY_TICKS);
                    m.remember(tick, format!("{name} died. Can't say I'm sorry."));
                    m.opinions.remove(&k);
                }
                None if w.owner == owner => {
                    m.feel(&format!("death:{name}"), format!("{name} died"), -0.05, tick + DAY_TICKS);
                    m.remember(tick, format!("{name} died."));
                }
                None => {}
            }
        }
    }

    // ------------------------------------------------------------ chats

    fn start_convos(&mut self) {
        let snaps = self.snaps();
        let tick = self.tick;
        let talking: Vec<Who> = self.social.convos.iter().flat_map(|c| [c.a, c.b]).collect();
        let free = |s: &Snap| !s.asleep && !s.busy && tick >= s.chat_after && !talking.contains(&s.who);
        let mut started = 0;
        let mut used: Vec<Who> = Vec::new();
        for i in 0..snaps.len() {
            if started >= 4 {
                break;
            }
            let a = &snaps[i];
            if !free(a) || used.contains(&a.who) {
                continue;
            }
            for b in snaps.iter().skip(i + 1) {
                if !free(b) || used.contains(&b.who) || dist(a.pos, b.pos) > CHAT_RANGE {
                    continue;
                }
                // Working people only stop for a word now and then; the lazy more often.
                let ok_a = a.leisure || self.social.rng.chance((1.0 - a.persona.diligent) * 0.2);
                let ok_b = b.leisure || self.social.rng.chance((1.0 - b.persona.diligent) * 0.2);
                let want = 0.1 * (0.5 + (a.persona.social + b.persona.social) * 0.5) * (1.4 - (a.company + b.company) * 0.5);
                if ok_a && ok_b && self.social.rng.chance(want) {
                    // The chattier one speaks first.
                    let (x, y) = if a.persona.social >= b.persona.social { (a.who, b.who) } else { (b.who, a.who) };
                    if self.begin_convo(x, y, None) {
                        used.push(a.who);
                        used.push(b.who);
                        started += 1;
                    }
                    break;
                }
            }
        }
    }

    /// Start a chat; `opener` is a first line already chosen (by a model).
    pub fn begin_convo(&mut self, a: Who, b: Who, opener: Option<String>) -> bool {
        if a == b || self.social.convo_of(a).is_some() || self.social.convo_of(b).is_some() {
            return false;
        }
        let (Some(pa), Some(pb)) = (self.person(a).cloned(), self.person(b).cloned()) else { return false };
        let ctx = TalkCtx {
            weather: if self.env.snowing() {
                "snow"
            } else if self.env.raining() {
                "rain"
            } else if self.env.fog > 0.4 {
                "fog"
            } else if self.env.temp < 2.0 {
                "cold"
            } else if self.env.temp > 28.0 {
                "hot"
            } else if self.env.wind > 0.5 {
                "wind"
            } else if self.env.temp < 10.0 {
                "chilly"
            } else {
                "fine"
            },
            group_a: self.group_name(a.owner),
            group_b: self.group_name(b.owner),
            strangers: a.owner != b.owner,
            has_needs: self.content.creatures.by_idx(pa.creature).needs.is_some(),
        };
        let (mut script, topic, tone) = make_script(&pa, &pb, &ctx, &mut self.social.rng);
        if let Some(o) = opener {
            script[0] = o;
        }
        let (na, nb) = (pa.name.clone(), pb.name.clone());
        let ticks = (script.len() as u64 * LINE_TICKS + 40) as u32;
        let hooked = pa.mind.hooked || pb.mind.hooked;
        // Hooked people wait for their model; give them longer.
        let ticks = if hooked { ticks + MODEL_WAIT as u32 } else { ticks };
        self.order_person(a, Order::Linger { ticks, label: format!("Chatting with {nb}") });
        self.order_person(b, Order::Linger { ticks, label: format!("Chatting with {na}") });
        let id = self.social.next_convo;
        self.social.next_convo += 1;
        self.social.convos.push(Convo { id, a, b, turn: 0, script, said: Vec::new(), next_at: self.tick, topic, tone, waiting_since: None, ready: None });
        true
    }

    fn advance_convos(&mut self) {
        let tick = self.tick;
        let mut i = 0;
        while i < self.social.convos.len() {
            let c = self.social.convos[i].clone();
            if tick < c.next_at {
                i += 1;
                continue;
            }
            let (Some(pa), Some(pb)) = (self.person(c.a), self.person(c.b)) else {
                self.social.convos.remove(i);
                continue;
            };
            let apart = dist(pa.pos, pb.pos) > CHAT_RANGE * 2.0 || pa.sleeping() || pb.sleeping();
            if c.turn >= c.script.len() || apart {
                let c = self.social.convos.remove(i);
                self.end_convo(c, apart);
                continue;
            }
            let (speaker, listener) = if c.turn.is_multiple_of(2) { (c.a, c.b) } else { (c.b, c.a) };
            let sp = self.person(speaker).unwrap();
            let li = self.person(listener).unwrap();
            let (sname, lname, hooked, suid) = (sp.name.clone(), li.name.clone(), sp.mind.hooked, sp.mind.uid);
            let any_hooked = hooked || li.mind.hooked;
            let mut line = None;
            let mut from_model = false;
            if hooked {
                let conv = &mut self.social.convos[i];
                if let Some(r) = conv.ready.take() {
                    line = Some(r);
                    from_model = true;
                } else if let Some(since) = c.waiting_since {
                    if tick - since < MODEL_WAIT {
                        i += 1;
                        continue;
                    }
                } else {
                    let (system, user) = self.chat_prompt(speaker, listener, &c);
                    if self.social.llm.ask(suid, Purpose::Chat { convo: c.id }, system, user) {
                        self.social.convos[i].waiting_since = Some(tick);
                        i += 1;
                        continue;
                    }
                }
            }
            let line = line.unwrap_or_else(|| c.script[c.turn].clone());
            let conv = &mut self.social.convos[i];
            conv.turn += 1;
            conv.waiting_since = None;
            conv.said.push((speaker, line.clone()));
            conv.next_at = tick + LINE_TICKS + (line.len() as u64).min(80);
            if let Some(p) = self.person_mut(speaker) {
                p.mind.speak(line.clone(), tick + LINE_TICKS * 2);
                if any_hooked {
                    p.mind.remember(tick, format!("You said to {lname}: \"{line}\""));
                }
            }
            if any_hooked && let Some(p) = self.person_mut(listener) {
                p.mind.remember(tick, format!("{sname} said to you: \"{line}\""));
            }
            self.social.push_feed(FeedLine { tick, who: speaker, name: sname, to: lname, text: line, model: from_model });
            i += 1;
        }
    }

    fn end_convo(&mut self, c: Convo, cut_short: bool) {
        let tick = self.tick;
        let (Some(pa), Some(pb)) = (self.person(c.a), self.person(c.b)) else { return };
        let (ua, ub, na, nb) = (pa.mind.uid, pb.mind.uid, pa.name.clone(), pb.name.clone());
        let (per_a, per_b) = (pa.mind.persona.clone(), pb.mind.persona.clone());
        let hooked = pa.mind.hooked || pb.mind.hooked;
        let first = pa.mind.opinions.get(&ub).is_none_or(|o| o.chats == 0);
        let chem = per_a.chemistry(&per_b);
        let words = if c.said.is_empty() { 0.3 } else { 1.0 };
        let mut msgs = Vec::new();
        for (me, other_uid, other, mine, theirs) in [(c.a, ub, nb.clone(), &per_a, &per_b), (c.b, ua, na.clone(), &per_b, &per_a)] {
            let jitter = self.social.rng.range_f32(-2.5, 2.5);
            let delta = (chem * 7.0 + (theirs.kind - 0.5) * 5.0 + (mine.kind - 0.5) * 2.0 + c.tone + jitter + if first { 3.0 } else { 0.0 }) * words;
            let Some(p) = self.person_mut(me) else { continue };
            let m = &mut p.mind;
            let before = m.opinion_of(other_uid);
            let after = m.adjust_opinion(other_uid, &other, delta, tick);
            if let Some(o) = m.opinions.get_mut(&other_uid) {
                o.chats += 1;
            }
            m.company = (m.company + 0.35 * words).min(1.0);
            m.chat_after = tick + (HOUR as f32 * (2.2 - mine.social * 1.6)) as u64;
            if delta >= -1.0 {
                m.feel(&format!("chat:{other}"), format!("Chatted with {other}"), 0.05, tick + DAY_TICKS / 4);
            } else {
                m.feel(&format!("chat:{other}"), format!("Argued with {other}"), -0.08, tick + DAY_TICKS / 3);
            }
            if !hooked {
                let what = if first { format!("Met {other} and talked about {}.", c.topic) } else { format!("Talked with {other} about {}.", c.topic) };
                m.remember(tick, what);
            }
            if before < 25.0 && after >= 25.0 {
                m.remember(tick, format!("{other} and I are friends now."));
                msgs.push((me.owner, format!("{} and {other} became friends.", p.name)));
            } else if before > -25.0 && after <= -25.0 {
                m.remember(tick, format!("I can't stand {other}."));
                msgs.push((me.owner, format!("{} can't stand {other} any more.", p.name)));
            }
        }
        // Friendship news about the player's people only, once per pair.
        msgs.dedup_by(|a, b| a.1.split(' ').next() == b.1.split(' ').nth(2));
        for (owner, m) in msgs.into_iter().take(1) {
            if owner == 0 || c.a.owner == 0 || c.b.owner == 0 {
                self.message(m);
            }
        }
        if !cut_short {
            for w in [c.a, c.b] {
                if self.person(w).is_some_and(|p| p.activity().starts_with("Chatting")) {
                    self.order_person(w, Order::Work);
                }
            }
        }
    }

    // ------------------------------------------------------------ plans

    fn pursue_intents(&mut self) {
        let tick = self.tick;
        for w in self.people() {
            let Some(p) = self.person(w) else { continue };
            let Some((intent, until)) = p.mind.intent.clone() else { continue };
            if tick > until {
                self.person_mut(w).unwrap().mind.intent = None;
                continue;
            }
            let Intent::Talk(uid) = intent else { continue };
            let Some(other) = self.find_uid(uid) else {
                self.person_mut(w).unwrap().mind.intent = None;
                continue;
            };
            let (here, idle) = (p.pos, p.job.is_none());
            let o = self.person(other).unwrap();
            let (there, at, oname) = (o.pos, o.tile(), o.name.clone());
            if dist(here, there) <= CHAT_RANGE {
                let opener = p.mind.say.as_ref().filter(|s| s.1 > tick).map(|s| s.0.clone());
                if !o.sleeping() && self.begin_convo(w, other, opener) {
                    self.person_mut(w).unwrap().mind.intent = None;
                }
            } else if idle {
                self.order_person(w, Order::Walk { to: at, label: format!("Going to see {oname}") });
            }
        }
    }

    // ------------------------------------------------------------ models

    fn hooked_thinking(&mut self) {
        let tick = self.tick;
        let every = (self.social.llm.cfg.think_every.max(2.0) * TPS) as u64;
        // Colonists first, then villagers nearest the camera: one model serves everyone.
        let view = self.view.map(|(a, b)| {
            let (ax, ay) = a.origin();
            let (bx, by) = b.origin();
            ((ax + bx) as f32 / 2.0, (ay + by) as f32 / 2.0)
        });
        let mut hooked: Vec<(f32, Who, u64)> = self
            .all_pawns()
            .filter(|(_, p)| p.mind.hooked && !p.drafted && p.mind.obey_until <= tick)
            .map(|(o, p)| {
                let far = view.map(|v| dist(p.pos, v)).unwrap_or(0.0);
                // Being asked something jumps the queue.
                let asked = if p.mind.request.is_some() { -1e6 } else { 0.0 };
                (if o == 0 { 0.0 } else { 1e5 } + far + asked, Who { owner: o, id: p.id }, p.mind.uid)
            })
            .collect();
        hooked.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, w, uid) in hooked {
            if !self.social.llm.has_room() {
                break;
            }
            if self.social.llm.busy(uid) || self.social.convo_of(w).is_some() || self.person(w).is_some_and(|p| p.sleeping()) {
                continue;
            }
            if tick < self.social.next_think.get(&uid).copied().unwrap_or(0) {
                continue;
            }
            let (system, user) = self.decide_prompt(w);
            // Colonists reason before they act; villagers answer straight away.
            if self.social.llm.ask_with(uid, Purpose::Decide, system, user, w.owner == 0) {
                self.social.next_think.insert(uid, tick + every);
            }
        }
    }

    /// Ask the model for a quick hello, to check the settings work.
    pub fn test_model(&mut self) {
        let sys = "You are a villager in a tiny world. Answer with JSON: {\"say\": \"...\"}".to_string();
        if self.social.llm.ask(0, Purpose::Test, sys, "Say hello in under ten words.".into()) {
            self.social.llm.status = format!("Asking {} at {} ...", self.social.llm.cfg.model, self.social.llm.cfg.url);
        }
    }

    fn apply_reply(&mut self, r: Reply) {
        let tick = self.tick;
        if r.purpose == Some(Purpose::Test) {
            if r.error.is_none() {
                self.social.llm.status = format!("It works! {} says: \"{}\" ({:.1}s)", self.social.llm.cfg.model, if r.say.is_empty() { &r.raw } else { &r.say }, r.secs);
            }
            return;
        }
        let Some(w) = self.find_uid(r.uid) else { return };
        if let Some(e) = &r.error {
            // Back off, and let a waiting chat use its stock line.
            let every = (self.social.llm.cfg.think_every.max(2.0) * TPS) as u64;
            self.social.next_think.insert(r.uid, tick + every * 3);
            if let Some(Purpose::Chat { convo }) = r.purpose
                && let Some(c) = self.social.convos.iter_mut().find(|c| c.id == convo)
            {
                c.waiting_since = Some(0);
            }
            if let Some(p) = self.person_mut(w) {
                p.mind.thought = format!("(the model didn't answer: {e})");
            }
            return;
        }
        if !r.thought.is_empty()
            && let Some(p) = self.person_mut(w)
        {
            // Shown, but not fed back: a model reading its own thoughts goes in circles.
            p.mind.thought = r.thought.clone();
        }
        match r.purpose {
            Some(Purpose::Chat { convo }) => {
                if let Some(c) = self.social.convos.iter_mut().find(|c| c.id == convo) {
                    c.ready = Some(if r.say.is_empty() { c.script.get(c.turn).cloned().unwrap_or_else(|| "...".into()) } else { r.say.clone() });
                    c.next_at = tick;
                }
                if !r.act.is_empty() {
                    // Carried out once the chat ends.
                    self.apply_act(w, &r.act, true);
                }
            }
            Some(Purpose::Decide) => {
                // Answering a request: agreeing or refusing is remembered by both.
                if let Some(req) = self.person_mut(w).and_then(|p| p.mind.request.take()) {
                    let agreed = r.act.to_lowercase().replace(['-', ' '], "_").starts_with(req.act.split(':').next().unwrap_or(""));
                    self.answer_request(w, &req, agreed, if r.say.is_empty() { None } else { Some(r.say.clone()) });
                    if !r.act.is_empty() {
                        self.apply_act(w, &r.act, false);
                    }
                    return;
                }
                let talk_to = r.act.to_lowercase().starts_with("talk");
                if !talk_to && !r.say.is_empty() {
                    self.say_aloud(w, &r.say, true);
                } else if talk_to
                    && !r.say.is_empty()
                    && let Some(p) = self.person_mut(w)
                {
                    // Saved as the opener when they meet.
                    p.mind.speak(r.say.clone(), tick + LINE_TICKS * 3);
                }
                if !r.act.is_empty() {
                    self.apply_act(w, &r.act, false);
                }
            }
            _ => {}
        }
    }

    /// Words said to no one in particular; everyone close hears them.
    fn say_aloud(&mut self, w: Who, text: &str, model: bool) {
        let tick = self.tick;
        let Some(p) = self.person_mut(w) else { return };
        p.mind.speak(text, tick + LINE_TICKS * 2);
        p.mind.remember(tick, format!("You said: \"{text}\""));
        let (name, pos) = (p.name.clone(), p.pos);
        let hearers: Vec<Who> = self.all_pawns().filter(|(o, q)| !(*o == w.owner && q.id == w.id) && !q.sleeping() && dist(q.pos, pos) <= HEAR_RANGE).map(|(o, q)| Who { owner: o, id: q.id }).collect();
        let mut to = String::new();
        for h in hearers {
            if let Some(q) = self.person_mut(h) {
                q.mind.remember(tick, format!("{name} said: \"{text}\""));
                if to.is_empty() {
                    to = q.name.clone();
                }
                if q.mind.hooked {
                    let uid = q.mind.uid;
                    self.social.next_think.insert(uid, tick + 20);
                }
            }
        }
        self.social.push_feed(FeedLine { tick, who: w, name, to: if to.is_empty() { "no one".into() } else { to }, text: text.to_string(), model });
    }

    /// Turn a model's "do" into an order.
    fn apply_act(&mut self, w: Who, act: &str, later: bool) {
        let tick = self.tick;
        let a = act.trim().to_lowercase().replace(['-', ' '], "_");
        let Some(p) = self.person(w) else { return };
        let here = p.tile();
        let (verb, mut arg) = match a.split_once(':') {
            Some((v, x)) => (v.trim_matches('_').to_string(), x.trim_matches('_').replace('_', " ")),
            None if a.starts_with("talk_to_") => ("talk_to".to_string(), a["talk_to_".len()..].replace('_', " ")),
            None => (a.clone(), String::new()),
        };
        let note = |g: &mut Game, s: String| {
            if let Some(p) = g.person_mut(w) {
                p.mind.remember(tick, s);
            }
        };
        let r = |g: &mut Game, n: i32| g.rng.range_i32(-n, n);
        let ok = match verb.as_str() {
            "work" => self.order_person(w, Order::Work),
            "eat" => self.order_person(w, Order::Eat),
            "sleep" => self.order_person(w, Order::Sleep),
            "warm_up" | "warm" | "warmup" => self.order_person(w, Order::Warm),
            "rest" | "wait" | "idle" => !later && self.order_person(w, Order::Linger { ticks: 30 * 12, label: "Resting".into() }),
            "wander" | "walk" => {
                let to = here.offset(r(self, 6), r(self, 6));
                self.order_person(w, Order::Walk { to, label: "Wandering".into() })
            }
            "explore" => {
                let to = here.offset(r(self, 20), r(self, 20));
                self.order_person(w, Order::Walk { to, label: "Exploring".into() })
            }
            "go_home" | "home" => {
                let to = self.home_of(w.owner);
                self.order_person(w, Order::Walk { to, label: "Heading home".into() })
            }
            "tell" | "ask" | "order" => {
                let (target, what) = arg.split_once(':').map(|(a, b)| (a.trim().to_string(), b.trim().replace(' ', "_"))).unwrap_or_else(|| ("everyone".into(), arg.replace(' ', "_")));
                return self.give_request(w, &target, &what);
            }
            "talk_to" | "talk" | "talk_with" => {
                let target = self.person_named(&arg, here, w);
                match target {
                    Some((_, uid, name, _)) if self.person(w).and_then(|p| p.mind.opinions.get(&uid)).is_some_and(|o| o.chats > 0 && tick < o.last + HOUR) => {
                        note(self, format!("You just talked with {name}; give it a while."));
                        false
                    }
                    Some((other, uid, name, at)) => {
                        if let Some(p) = self.person_mut(w) {
                            p.mind.intent = Some((Intent::Talk(uid), tick + HOUR * 3));
                        }
                        arg = name.clone();
                        let near = self.person(w).zip(self.person(other)).is_some_and(|(a, b)| dist(a.pos, b.pos) <= CHAT_RANGE);
                        near || later || self.order_person(w, Order::Walk { to: at, label: format!("Going to talk to {name}") })
                    }
                    None => {
                        note(self, format!("Wanted to talk to {arg}, but they aren't around."));
                        false
                    }
                }
            }
            _ => false,
        };
        if ok {
            note(self, format!("Decided to {}.", if arg.is_empty() { verb.replace('_', " ") } else { format!("{} {arg}", verb.replace('_', " ")) }));
        }
    }

    /// Ask someone (or everyone) within earshot to do something. Each one who
    /// hears decides: people thinking with a model are asked, the others go by
    /// their nature, mood and what they think of the speaker.
    fn give_request(&mut self, w: Who, target: &str, act: &str) {
        let tick = self.tick;
        let verb = act.split(':').next().unwrap_or("");
        if act.is_empty() || ["tell", "ask", "order"].contains(&verb) || !crate::llm::is_action(act) {
            return;
        }
        let Some(p) = self.person(w) else { return };
        let (name, uid, pos) = (p.name.clone(), p.mind.uid, p.pos);
        let all = ["everyone", "all", "everybody", ""].contains(&target.to_lowercase().as_str());
        let want = target.to_lowercase();
        let hearers: Vec<Who> = self
            .all_pawns()
            .filter(|(o, q)| !(*o == w.owner && q.id == w.id) && !q.sleeping() && dist(q.pos, pos) <= HEAR_RANGE)
            .filter(|(_, q)| all || q.name.to_lowercase().starts_with(&want) || want.starts_with(&q.name.to_lowercase()))
            .map(|(o, q)| Who { owner: o, id: q.id })
            .collect();
        // Make sure it was said out loud.
        if self.person(w).is_some_and(|p| p.mind.say.as_ref().is_none_or(|(_, until)| *until < tick)) {
            let who = if all { "everyone".to_string() } else { target.to_string() };
            self.say_aloud(w, &format!("{}, {}!", capitalize(&who), act.replace(['_', ':'], " ")), false);
        }
        if let Some(p) = self.person_mut(w) {
            p.mind.remember(tick, format!("You asked {} to {}.", if all { "everyone nearby" } else { target }, act.replace('_', " ")));
        }
        let req = pw_colony::mind::Request { from: uid, name: name.clone(), act: act.to_string(), at: tick };
        for h in hearers {
            let Some(q) = self.person(h) else { continue };
            if q.mind.hooked && !q.drafted && q.mind.obey_until <= tick {
                let quid = q.mind.uid;
                if let Some(q) = self.person_mut(h) {
                    q.mind.request = Some(req.clone());
                }
                self.social.think_now(quid);
            } else if !q.drafted && q.mind.obey_until <= tick {
                // Their own judgement: nature, mood, and how they feel about the speaker.
                let m = &q.mind;
                let mut yes = 0.3 + m.persona.diligent * 0.25 + m.persona.kind * 0.15 + m.opinion_of(uid) / 200.0 + (m.mood - 0.5) * 0.3;
                if h.owner == w.owner {
                    yes += 0.15;
                }
                let agreed = self.social.rng.chance(yes.clamp(0.05, 0.95));
                self.answer_request(h, &req, agreed, None);
                if agreed {
                    self.apply_act(h, act, false);
                }
            }
        }
    }

    /// Someone says yes or no to a request; both remember it.
    fn answer_request(&mut self, w: Who, req: &pw_colony::mind::Request, agreed: bool, words: Option<String>) {
        let tick = self.tick;
        let Some(p) = self.person_mut(w) else { return };
        let what = req.act.replace(['_', ':'], " ");
        p.mind.remember(tick, if agreed { format!("{} asked you to {what}; you agreed.", req.name) } else { format!("{} asked you to {what}; you refused.", req.name) });
        let name = p.name.clone();
        let line = words.unwrap_or_else(|| {
            let yes = ["Alright.", "On it.", "Sure thing.", "If you say so."];
            let no = ["Not now.", "Do it yourself.", "Maybe later.", "I'd rather not."];
            let v = if agreed { &yes } else { &no };
            format!("{} {}", v[(tick as usize + w.id as usize) % v.len()], req.name)
        });
        self.say_aloud(w, &line, false);
        if let Some(asker) = self.find_uid(req.from)
            && let Some(a) = self.person_mut(asker)
        {
            a.mind.remember(tick, if agreed { format!("{name} agreed to {what}.") } else { format!("{name} refused to {what}.") });
        }
    }

    /// The person called `name` closest to `near` (prefix match, any case).
    fn person_named(&self, name: &str, near: TilePos, not: Who) -> Option<(Who, u64, String, TilePos)> {
        let want = name.trim().to_lowercase();
        if want.is_empty() {
            return None;
        }
        self.all_pawns()
            .filter(|(o, p)| !(*o == not.owner && p.id == not.id))
            .filter(|(_, p)| {
                let n = p.name.to_lowercase();
                n == want || n.starts_with(&want) || want.starts_with(&n)
            })
            .min_by_key(|(_, p)| p.tile().chebyshev(near))
            .map(|(o, p)| (Who { owner: o, id: p.id }, p.mind.uid, p.name.clone(), p.tile()))
    }

    // ------------------------------------------------------------ prompts

    fn system_prompt(&self, w: Who) -> String {
        let p = self.person(w).unwrap();
        let traits = p.mind.persona.traits();
        let traits = if traits.is_empty() { "fairly ordinary".to_string() } else { traits.join(", ") };
        format!(
            "You are {name}, a person living in Partiworld, a tiny pixel-art world of colonists, tribes and wild animals. \
             You are {traits}. You live in {group}. You only know what the notes tell you. \
             Stay in character, speak plainly like a villager, keep anything you say under 20 words, and don't repeat yourself.\n\
             Answer with one JSON object and nothing else, with exactly these three keys:\n\
             \"thought\": your private thought, one short sentence.\n\
             \"say\": the exact words you speak out loud, or \"\" to stay quiet. Words only, not a description of what you do.\n\
             \"do\": one of: work, eat, sleep, warm_up, rest, wander, explore, go_home, talk_to:<name of someone near you>, \
             tell:<name or everyone>:<one of those actions> (ask people who can hear you to do something; they may refuse)\n\
             You are free: do whatever you think is best for you.\n\
             Only an example of the shape (use your own words): {example}",
            name = p.name,
            group = self.group_name(w.owner),
            example = EXAMPLES[(p.mind.uid % EXAMPLES.len() as u64) as usize],
        )
    }

    /// Everything a person knows right now, as notes for a model.
    pub fn situation(&self, w: Who) -> String {
        let Some(p) = self.person(w) else { return String::new() };
        let c = &self.content;
        let m = &p.mind;
        let (day, h, mi) = self.day_time();
        let weather = self.env.weather_def(&c.climate).map(|w| w.name.to_lowercase()).unwrap_or_default();
        let night = if self.light().dark > 0.62 { ", night" } else { "" };
        let mut s = format!("Time: {} day {day}, {h:02}:{mi:02}{night}. Weather: {weather}, {:.0} C.\n", self.season_name(), self.env.temp);
        let mut feelings: Vec<&pw_colony::mind::Moodlet> = m.moodlets.iter().collect();
        feelings.sort_by(|a, b| b.value.abs().total_cmp(&a.value.abs()));
        let fl: Vec<String> = feelings.iter().take(5).map(|x| format!("{} ({:+.0})", x.label, x.value * 100.0)).collect();
        s += &format!("You feel {} (mood {:.0}%).", m.mood_word(), m.mood * 100.0);
        if !fl.is_empty() {
            s += &format!(" Because: {}.", fl.join(", "));
        }
        s += "\n";
        if c.creatures.by_idx(p.creature).needs.is_some() {
            let n = &p.needs;
            s += &format!("Body: food {:.0}%, rest {:.0}%, warmth {:.0}%, health {:.0}%.\n", n.food * 100.0, n.rest * 100.0, n.warmth * 100.0, n.health * 100.0);
        }
        s += &format!("Right now you are: {}", p.activity().to_lowercase());
        if let Some((item, n)) = p.carrying {
            s += &format!(", carrying {n} {}", c.items.by_idx(item).name.to_lowercase());
        }
        s += ".\n";
        let here = p.tile();
        let mut near: Vec<(i32, String)> = self
            .all_pawns()
            .filter(|(o, q)| !(*o == w.owner && q.id == w.id))
            .map(|(o, q)| (q.tile().chebyshev(here), o, q))
            .filter(|x| x.0 <= NEAR_TILES)
            .map(|(d, o, q)| {
                let rel = match m.opinions.get(&q.mind.uid) {
                    Some(op) => op.word().to_string(),
                    None if o == w.owner => "neighbour you barely know".into(),
                    None => format!("stranger from {}", self.group_name(o)),
                };
                let doing = if q.sleeping() { "asleep".to_string() } else { q.activity().to_lowercase() };
                (d, format!("{} ({rel}, {d} tiles away, {doing})", q.name))
            })
            .collect();
        near.sort_by_key(|x| x.0);
        let nl: Vec<String> = near.into_iter().take(6).map(|x| x.1).collect();
        s += &format!("People near you: {}.\n", if nl.is_empty() { "nobody".to_string() } else { nl.join("; ") });
        let mut ops: Vec<&pw_colony::mind::Opinion> = m.opinions.values().filter(|o| o.value.abs() >= 15.0).collect();
        ops.sort_by(|a, b| b.value.abs().total_cmp(&a.value.abs()));
        if !ops.is_empty() {
            let ol: Vec<String> = ops.iter().take(5).map(|o| format!("{} ({})", o.name, o.word())).collect();
            s += &format!("People you know: {}.\n", ol.join(", "));
        }
        if let Some(r) = &m.request {
            s += &format!(
                "{} just asked you to {}. You can do it (put that in \"do\") or refuse; say your answer out loud.\n",
                r.name,
                r.act.replace(['_', ':'], " ")
            );
        }
        let mem: Vec<&pw_colony::mind::Memory> = m.memories.iter().rev().take(8).collect();
        if !mem.is_empty() {
            s += "Recent memories, newest last:\n";
            for x in mem.into_iter().rev() {
                s += &format!("- {}\n", x.text);
            }
        }
        s
    }

    fn decide_prompt(&self, w: Who) -> (String, String) {
        (self.system_prompt(w), format!("{}\nWhat do you do now?", self.situation(w)))
    }

    fn chat_prompt(&self, speaker: Who, listener: Who, c: &Convo) -> (String, String) {
        let other = self.person(listener).map(|p| (p.name.clone(), p.mind.uid)).unwrap_or_default();
        let rel = self.person(speaker).map(|p| relation_word(p.mind.opinion_of(other.1))).unwrap_or("acquaintance");
        let mut u = self.situation(speaker);
        u += &format!("\nYou are talking with {} ({rel}).", other.0);
        if c.said.is_empty() {
            u += " You speak first.";
        } else {
            u += " The conversation so far:\n";
            for (who, line) in &c.said {
                let n = if *who == speaker { "You".to_string() } else { other.0.clone() };
                u += &format!("{n}: {line}\n");
            }
        }
        let before: Vec<&str> = self
            .person(speaker)
            .map(|p| p.mind.memories.iter().rev().filter_map(|m| m.text.strip_prefix("You said")).take(4).collect())
            .unwrap_or_default();
        if !before.is_empty() && c.said.is_empty() {
            u += &format!("\nThings you said lately (say something new): {}", before.join(" | "));
        }
        u += "\nWhat do you say next? Put it in \"say\". \"do\" can be empty.";
        (self.system_prompt(speaker), u)
    }
}

// ---------------------------------------------------------------- stock lines

/// The voice in someone's head, from what weighs on them most.
fn inner_voice(m: &pw_colony::Mind, n: &pw_colony::Needs, has_needs: bool, label: &str, night: bool, rng: &mut Rng) -> String {
    let p = &m.persona;
    let mut c: Vec<String> = Vec::new();
    if has_needs {
        if n.food <= 0.05 {
            c.push("I need to eat something. Anything.".into());
        } else if n.food < 0.3 {
            c.push("My stomach won't stop growling.".into());
        }
        if n.warmth < 0.4 {
            c.push("So cold... I need a fire.".into());
        }
        if n.rest < 0.25 {
            c.push("My eyes keep closing.".into());
        }
        if n.health < 0.5 {
            c.push("Everything hurts.".into());
        }
    }
    if let Some(g) = m.moodlets.iter().find(|x| x.key.starts_with("death:")) {
        c.push(format!("I keep thinking about {}.", g.key.trim_start_matches("death:")));
    }
    if !c.is_empty() {
        return c.swap_remove(rng.below(c.len()));
    }
    if m.company < 0.25 {
        c.push(if p.social > 0.5 { "I haven't talked to anyone in ages.".into() } else { "Quiet. Just how I like it... mostly.".into() });
    }
    if m.mood < 0.25 {
        c.push("Nothing is going right.".into());
        if let Some(e) = m.worst_enemy() {
            c.push(format!("If {} says one more word to me...", e.name));
        }
    }
    if let Some(f) = m.best_friend()
        && rng.chance(0.4)
    {
        c.push(format!("I wonder what {} is up to.", f.name));
    }
    if let Some(w) = work_phrase(label) {
        if p.diligent < 0.35 {
            c.push(format!("Do I really have to keep {w}?"));
        } else if p.diligent > 0.65 {
            c.push(format!("{} - good honest work.", capitalize(&w)));
        }
    }
    if night && label.starts_with("Sleeping") {
        c.push("Zzz...".into());
    }
    if p.curious > 0.66 {
        c.push("I wonder what lies past those hills.".into());
    }
    if p.brave < 0.3 {
        c.push("Was that a wolf? I'm sure I heard something.".into());
    }
    if m.mood > 0.75 {
        c.push("Life's good.".into());
    }
    if c.is_empty() {
        c.push(if m.mood > 0.5 { "Not a bad day.".into() } else { "Another day.".into() });
    }
    c.swap_remove(rng.below(c.len()))
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

struct TalkCtx {
    weather: &'static str,
    group_a: String,
    group_b: String,
    strangers: bool,
    has_needs: bool,
}

/// A short exchange between `a` and `b`: lines (a first, then alternating),
/// what it was about, and an opinion nudge from how it went.
fn make_script(a: &Pawn, b: &Pawn, ctx: &TalkCtx, rng: &mut Rng) -> (Vec<String>, &'static str, f32) {
    let (ma, mb) = (&a.mind, &b.mind);
    let (an, bn) = (a.name.as_str(), b.name.as_str());
    let op_ab = ma.opinion_of(mb.uid);
    let op_ba = mb.opinion_of(ma.uid);
    let kind_b = mb.persona.kind;
    let mut lines: Vec<String> = Vec::new();

    // Strangers introduce themselves.
    if ctx.strangers && !ma.opinions.contains_key(&mb.uid) {
        lines.push(format!("Hello there. I'm {an}, from {}.", ctx.group_a));
        lines.push(if kind_b > 0.45 { format!("Well met, {an}. I'm {bn} of {}.", ctx.group_b) } else { "You're a long way from home.".into() });
        if ma.persona.curious > 0.5 {
            lines.push("What's it like where you live?".into());
            lines.push(pick(rng, &["Plenty of trees, never enough food.", "Busy. Always building something.", "Quiet. Mostly."]).into());
        } else {
            lines.push("Well. Safe travels.".into());
        }
        return (lines, "where they're from", 1.0);
    }

    // Greeting, by how they feel about each other.
    if rng.chance(0.6) {
        lines.push(match op_ab {
            v if v >= 40.0 => format!("{}! {}", bn, pick(rng, &["Good to see you.", "There you are!", "How are you holding up?"])),
            v if v <= -30.0 => format!("Oh. {bn}."),
            _ => pick(rng, &["Hey.", "Morning.", "Hello."]).to_string(),
        });
        lines.push(match op_ba {
            v if v >= 40.0 => format!("{an}! Always a pleasure."),
            v if v <= -30.0 => "What do you want?".into(),
            _ => format!("Hi, {an}."),
        });
    }

    // Enemies and grumps snipe.
    if op_ab <= -40.0 && ma.persona.kind < 0.45 && rng.chance(0.6) {
        lines.push(format!("Stay out of my way, {bn}."));
        lines.push(if kind_b > 0.6 { "I don't know what I did to you.".into() } else { "Gladly.".into() });
        return (lines, "their feud", -6.0);
    }

    // Pick what to talk about from what's on their minds.
    let mut topics: Vec<(&'static str, f32)> = vec![("the weather", 0.8), ("plans", 1.0)];
    if ctx.has_needs {
        if a.needs.food < 0.35 {
            topics.push(("food", 3.0));
        }
        if a.needs.warmth < 0.5 {
            topics.push(("the cold", 3.0));
        }
        if a.needs.rest < 0.3 {
            topics.push(("being tired", 2.0));
        }
    }
    if work_phrase(&ma.last_work).is_some() {
        topics.push(("work", 1.5));
    }
    if ma.opinions.values().any(|o| o.value.abs() >= 25.0 && o.name != bn) {
        topics.push(("other people", 1.5));
    }
    if ma.moodlets.iter().any(|x| x.key.starts_with("death:")) {
        topics.push(("a death", 4.0));
    }
    if ma.persona.curious > 0.6 {
        topics.push(("what's out there", 1.0));
    }
    if ma.mood > 0.7 || ma.mood < 0.3 {
        topics.push(("how things are going", 1.2));
    }
    let total: f32 = topics.iter().map(|t| t.1).sum();
    let mut roll = rng.f32() * total;
    let mut topic = topics[0].0;
    for t in &topics {
        if roll < t.1 {
            topic = t.0;
            break;
        }
        roll -= t.1;
    }
    let mut tone = 0.0;
    match topic {
        "food" => {
            lines.push(pick(rng, &["I'm starving. Is there anything to eat?", "My stomach won't stop growling."]).into());
            if kind_b > 0.55 {
                lines.push("I'll keep an eye out for berries for you.".into());
                tone = 2.0;
            } else {
                lines.push("Join the club.".into());
            }
        }
        "the cold" => {
            lines.push(pick(rng, &["I can't feel my fingers.", "This cold gets into my bones."]).into());
            lines.push(pick(rng, &["We need a fire going.", "Stand closer, it's warmer.", "Stop complaining, it's bracing."]).into());
        }
        "being tired" => {
            lines.push("I'm dead on my feet.".into());
            lines.push(if kind_b > 0.5 { "Get some sleep, you look awful.".into() } else { "Then stop yawning at me.".into() });
        }
        "work" => {
            let w = work_phrase(&ma.last_work).unwrap_or_default();
            lines.push(if ma.persona.diligent > 0.5 { format!("Been {w}. Good to get it done.") } else { format!("Been {w} all day. My arms ache.") });
            lines.push(if mb.persona.diligent > 0.6 {
                "Someone has to do it.".into()
            } else if mb.persona.diligent < 0.4 {
                tone = 1.5;
                "Let's take a break, then.".into()
            } else {
                "Same here.".into()
            });
        }
        "other people" => {
            let o = ma.opinions.values().filter(|o| o.value.abs() >= 25.0 && o.name != bn).max_by(|x, y| x.value.abs().total_cmp(&y.value.abs())).unwrap();
            let third = o.name.clone();
            let theirs = mb.opinions.values().find(|x| x.name == third).map(|x| x.value).unwrap_or(0.0);
            if o.value > 0.0 {
                lines.push(format!("Have you talked to {third}? Good soul."));
                if theirs > 10.0 {
                    lines.push(format!("{third}? The best."));
                    tone = 3.0;
                } else if theirs < -10.0 {
                    lines.push(format!("{third}? We'll have to disagree."));
                    tone = -2.0;
                } else {
                    lines.push("I don't know them well.".into());
                }
            } else {
                lines.push(format!("{third} is getting on my nerves."));
                if theirs < -10.0 {
                    lines.push("Tell me about it!".into());
                    tone = 3.0;
                } else if theirs > 10.0 {
                    lines.push(format!("Hey, {third} is my friend."));
                    tone = -3.0;
                } else {
                    lines.push("Hm. Give them a chance.".into());
                }
            }
        }
        "a death" => {
            let g = ma.moodlets.iter().find(|x| x.key.starts_with("death:")).unwrap();
            let who = g.key.trim_start_matches("death:").to_string();
            lines.push(format!("I keep thinking about {who}."));
            lines.push(if kind_b > 0.4 { "Me too. It's not the same without them.".into() } else { "It happens. We go on.".into() });
            tone = 2.0;
        }
        "what's out there" => {
            lines.push(pick(rng, &["Ever wonder what's past those hills?", "I want to see where the river goes."]).into());
            lines.push(if mb.persona.curious > 0.55 { "Every day! Let's go one day.".into() } else { "Not really. I like it here.".into() });
            tone = if mb.persona.curious > 0.55 { 2.0 } else { -0.5 };
        }
        "how things are going" => {
            if ma.mood > 0.7 {
                lines.push("Things are going well, aren't they?".into());
                lines.push(if mb.mood > 0.5 { "They really are.".into() } else { "If you say so.".into() });
            } else {
                lines.push("I don't know how much more of this I can take.".into());
                lines.push(if kind_b > 0.5 {
                    tone = 3.0;
                    "Hang in there. I'm here.".into()
                } else {
                    "We all have it hard.".into()
                });
            }
        }
        "plans" => {
            lines.push(pick(rng, &["What are you up to?", "Busy today?", "Keeping busy?"]).into());
            lines.push(match work_phrase(&mb.last_work) {
                Some(w) => format!("{}. {}", capitalize(&w), pick(rng, &["Slow going.", "Nearly done.", "It never ends.", "Not bad work."])),
                None => pick(rng, &["Nothing much. Taking it easy.", "Just stretching my legs.", "Looking for something to do."]).into(),
            });
        }
        _ => {
            let (q, r): (&[&str], &[&str]) = match ctx.weather {
                "rain" => (&["This rain won't let up.", "Soaked to the bone again.", "Listen to that rain."], &["Good for the berries, at least.", "Better than a drought.", "Let's get under a roof."]),
                "snow" => (&["Snow again...", "Look at all this snow.", "My boots are full of snow."], &["I like how quiet it gets.", "Watch your step.", "Spring can't come soon enough."]),
                "fog" => (&["Can't see a thing in this fog.", "Strange fog today."], &["Stay close to home.", "Anything could be out there."]),
                "cold" => (&["Cold one today.", "My breath is freezing."], &["Stay near the fire.", "Keep moving, it helps."]),
                "chilly" => (&["A bit chilly, isn't it?", "There's a nip in the air."], &["Bracing, I'd say.", "I could use a fire."]),
                "hot" => (&["Too hot to work.", "This sun is cooking me."], &["Find some shade.", "Drink something."]),
                "wind" => (&["Hear that wind?", "This wind nearly took my hat."], &["Hold on to something.", "Storm coming, maybe."]),
                _ => (&["Nice day, isn't it?", "Lovely weather.", "What a sky."], &["Too nice to be working.", "Can't complain.", "Make the most of it."]),
            };
            lines.push(pick(rng, q).into());
            lines.push(pick(rng, r).into());
        }
    }
    if rng.chance(0.35) {
        lines.push(pick(rng, &["Anyway, back to it.", "See you around.", "Take care."]).into());
    }
    (lines, topic, tone)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_phrases_skip_idle_labels() {
        assert_eq!(work_phrase("Chopping oak tree").as_deref(), Some("chopping oak tree"));
        assert!(work_phrase("Chatting with Bo").is_none());
        assert!(work_phrase("Idle").is_none());
    }
}
