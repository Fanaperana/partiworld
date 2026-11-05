//! What goes on inside a person: personality, mood, memories and what they
//! think of the people they've met. The colony job system reads it (lazy
//! people work slower, unhappy people drag their feet); the game's social
//! system writes it (chats, deaths, weather), and an optional language model
//! can take over a person's choices and words.

use std::collections::{BTreeMap, VecDeque};

use pw_core::Rng;
use serde::{Deserialize, Serialize};

/// Most memories kept per person.
const MEMORIES: usize = 24;
/// Most people one person keeps an opinion of.
const OPINIONS: usize = 24;

/// Personality on five axes, each 0..1. Extremes become named traits.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Persona {
    /// Seeks out company and talks a lot (low: shy).
    pub social: f32,
    /// Warm and forgiving (low: grumpy, quick to take offence).
    pub kind: f32,
    /// Works hard and fast (low: lazy).
    pub diligent: f32,
    /// Faces danger (low: timid).
    pub brave: f32,
    /// Explores and asks questions (low: a homebody).
    pub curious: f32,
}

impl Default for Persona {
    fn default() -> Self {
        Self { social: 0.5, kind: 0.5, diligent: 0.5, brave: 0.5, curious: 0.5 }
    }
}

/// (high name, low name) for each axis, in field order.
pub const TRAIT_PAIRS: [(&str, &str); 5] = [("chatty", "shy"), ("kind", "grumpy"), ("hardworking", "lazy"), ("brave", "timid"), ("curious", "homebody")];

impl Persona {
    pub fn random(rng: &mut Rng) -> Self {
        // Two dice: most people are middling, a few are extreme.
        let mut roll = || ((rng.f32() + rng.f32()) * 0.5 * 1.3 - 0.15).clamp(0.0, 1.0);
        Self { social: roll(), kind: roll(), diligent: roll(), brave: roll(), curious: roll() }
    }
    pub fn axes(&self) -> [f32; 5] {
        [self.social, self.kind, self.diligent, self.brave, self.curious]
    }
    /// Named traits for the axes that stand out.
    pub fn traits(&self) -> Vec<&'static str> {
        self.axes()
            .iter()
            .zip(TRAIT_PAIRS)
            .filter_map(|(&v, (hi, lo))| {
                if v >= 0.66 {
                    Some(hi)
                } else if v <= 0.34 {
                    Some(lo)
                } else {
                    None
                }
            })
            .collect()
    }
    /// How well two people get along, -1..1: kindness helps, two grumps clash,
    /// a curious and a homebody bore each other.
    pub fn chemistry(&self, o: &Persona) -> f32 {
        let warmth = (self.kind + o.kind) - 1.0;
        let alike = 1.0 - ((self.curious - o.curious).abs() + (self.social - o.social).abs()) * 0.5;
        (warmth * 0.7 + (alike - 0.5) * 0.6).clamp(-1.0, 1.0)
    }
}

/// A passing feeling that lifts or sinks the mood until it expires.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Moodlet {
    /// Same key replaces the old one (no stacking "ate a meal" ten times).
    pub key: String,
    pub label: String,
    pub value: f32,
    pub until: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Memory {
    pub tick: u64,
    pub text: String,
}

/// What one person thinks of another.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Opinion {
    pub name: String,
    /// -100 (hates) .. 100 (loves).
    pub value: f32,
    pub chats: u32,
    pub last: u64,
}

impl Opinion {
    pub fn word(&self) -> &'static str {
        relation_word(self.value)
    }
}

pub fn relation_word(v: f32) -> &'static str {
    match v {
        v if v >= 60.0 => "close friend",
        v if v >= 25.0 => "friend",
        v if v > -25.0 => "acquaintance",
        v if v > -60.0 => "rival",
        _ => "enemy",
    }
}

/// What a person (or a model driving them) has decided to do next.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Intent {
    Work,
    Eat,
    Sleep,
    Warm,
    Rest,
    Wander,
    Explore,
    GoHome,
    /// Walk over and talk to this person (uid).
    Talk(u64),
}

impl Intent {
    pub fn label(&self) -> &'static str {
        match self {
            Intent::Work => "work",
            Intent::Eat => "eat",
            Intent::Sleep => "sleep",
            Intent::Warm => "warm up",
            Intent::Rest => "rest",
            Intent::Wander => "wander",
            Intent::Explore => "explore",
            Intent::GoHome => "go home",
            Intent::Talk(_) => "talk",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mind {
    /// Unique across the whole world (pawn ids are only unique per colony).
    pub uid: u64,
    pub persona: Persona,
    /// 0 miserable .. 1 elated. Recomputed now and then.
    pub mood: f32,
    pub moodlets: Vec<Moodlet>,
    /// 0 lonely .. 1 had plenty of company.
    pub company: f32,
    /// By uid.
    pub opinions: BTreeMap<u64, Opinion>,
    pub memories: VecDeque<Memory>,
    /// Words shown above the head until a tick.
    pub say: Option<(String, u64)>,
    /// Inner voice, shown in the character panel.
    pub thought: String,
    /// A plan that overrides the usual job picking for a while.
    pub intent: Option<(Intent, u64)>,
    /// No new chat before this tick.
    pub chat_after: u64,
    /// The last job label, for small talk ("Chopping all day...").
    pub last_work: String,
    /// A language model chooses this person's words and plans.
    pub hooked: bool,
}

impl Default for Mind {
    fn default() -> Self {
        Self {
            uid: 0,
            persona: Persona::default(),
            mood: 0.6,
            moodlets: Vec::new(),
            company: 0.7,
            opinions: BTreeMap::new(),
            memories: VecDeque::new(),
            say: None,
            thought: String::new(),
            intent: None,
            chat_after: 0,
            last_work: String::new(),
            hooked: false,
        }
    }
}

impl Mind {
    pub fn new(rng: &mut Rng) -> Self {
        Self { uid: rng.next_u64() | 1, persona: Persona::random(rng), company: 0.5 + rng.f32() * 0.4, ..Default::default() }
    }
    /// A mind from a seed, leaving the world's dice alone.
    pub fn from_seed(seed: u64) -> Self {
        Self::new(&mut Rng::new(pw_core::splitmix64(seed)))
    }

    pub fn remember(&mut self, tick: u64, text: impl Into<String>) {
        let text = text.into();
        if self.memories.back().is_some_and(|m| m.text == text) {
            return;
        }
        self.memories.push_back(Memory { tick, text });
        while self.memories.len() > MEMORIES {
            self.memories.pop_front();
        }
    }

    /// Add or refresh a feeling.
    pub fn feel(&mut self, key: &str, label: impl Into<String>, value: f32, until: u64) {
        let label = label.into();
        if let Some(m) = self.moodlets.iter_mut().find(|m| m.key == key) {
            m.label = label;
            m.value = value;
            m.until = until.max(m.until);
        } else {
            self.moodlets.push(Moodlet { key: key.into(), label, value, until });
        }
    }

    pub fn opinion_of(&self, uid: u64) -> f32 {
        self.opinions.get(&uid).map(|o| o.value).unwrap_or(0.0)
    }

    /// Shift the opinion of someone; forget the least known when full.
    pub fn adjust_opinion(&mut self, uid: u64, name: &str, by: f32, tick: u64) -> f32 {
        if !self.opinions.contains_key(&uid)
            && self.opinions.len() >= OPINIONS
            && let Some(k) = self.opinions.iter().min_by(|a, b| a.1.value.abs().total_cmp(&b.1.value.abs())).map(|(k, _)| *k)
        {
            self.opinions.remove(&k);
        }
        let o = self.opinions.entry(uid).or_insert(Opinion { name: name.to_string(), value: 0.0, chats: 0, last: tick });
        o.name = name.to_string();
        o.value = (o.value + by).clamp(-100.0, 100.0);
        o.last = tick;
        o.value
    }

    pub fn speak(&mut self, text: impl Into<String>, until: u64) {
        self.say = Some((text.into(), until));
    }

    pub fn saying(&self, tick: u64) -> Option<&str> {
        self.say.as_ref().filter(|s| s.1 > tick && !s.0.is_empty()).map(|s| s.0.as_str())
    }

    /// Best friend and worst enemy, if any stand out.
    pub fn best_friend(&self) -> Option<&Opinion> {
        self.opinions.values().filter(|o| o.value >= 25.0).max_by(|a, b| a.value.total_cmp(&b.value))
    }
    pub fn worst_enemy(&self) -> Option<&Opinion> {
        self.opinions.values().filter(|o| o.value <= -25.0).min_by(|a, b| a.value.total_cmp(&b.value))
    }

    pub fn mood_word(&self) -> &'static str {
        match self.mood {
            m if m >= 0.8 => "happy",
            m if m >= 0.6 => "content",
            m if m >= 0.4 => "okay",
            m if m >= 0.2 => "unhappy",
            _ => "miserable",
        }
    }

    /// How fast this person works: hard workers and happy people faster.
    pub fn work_rate(&self) -> f32 {
        let mood = if self.mood < 0.25 { 0.8 } else if self.mood > 0.75 { 1.1 } else { 1.0 };
        (0.75 + self.persona.diligent * 0.5) * mood
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opinions_clamp_and_forget() {
        let mut m = Mind::default();
        for i in 0..40u64 {
            m.adjust_opinion(i + 1, "x", i as f32, 0);
        }
        assert!(m.opinions.len() <= OPINIONS);
        assert_eq!(m.adjust_opinion(1000, "y", 500.0, 0), 100.0);
    }

    #[test]
    fn feelings_replace_by_key() {
        let mut m = Mind::default();
        m.feel("meal", "Ate", 0.1, 10);
        m.feel("meal", "Ate well", 0.2, 20);
        assert_eq!(m.moodlets.len(), 1);
        assert_eq!(m.moodlets[0].value, 0.2);
    }

    #[test]
    fn kind_people_get_along() {
        let a = Persona { kind: 0.9, ..Default::default() };
        let b = Persona { kind: 0.1, ..Default::default() };
        assert!(a.chemistry(&a) > b.chemistry(&b));
    }
}
