//! Seasons, time of day, temperature and weather, driven by `ClimateDef` data.

use pw_core::*;
use serde::{Deserialize, Serialize};

use crate::DAY_TICKS;

/// Ticks in one in-game hour.
pub const HOUR_TICKS: u64 = DAY_TICKS / 24;

/// The world's weather and climate state (saved with the game).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Env {
    /// Current weather id (by id so saves survive mod changes).
    pub weather: String,
    pub weather_until: u64,
    /// Direction the wind blows toward, radians.
    pub wind_dir: f32,
    /// Current values; they ease toward the weather's targets over about an hour.
    pub wind: f32,
    pub wind_target: f32,
    pub precip: f32,
    pub clouds: f32,
    pub fog: f32,
    /// Ground wetness and snow cover, 0..1.
    pub wetness: f32,
    pub snow: f32,
    /// Outdoor temperature at the colony, C.
    pub temp: f32,
    /// Temperature offset of the colony's biome, C.
    pub biome_temp: f32,
}

impl Default for Env {
    fn default() -> Self {
        Self {
            weather: String::new(),
            weather_until: 0,
            wind_dir: 0.7,
            wind: 0.1,
            wind_target: 0.1,
            precip: 0.0,
            clouds: 0.0,
            fog: 0.0,
            wetness: 0.0,
            snow: 0.0,
            temp: 15.0,
            biome_temp: 0.0,
        }
    }
}

/// Season values: each season starts at its own values and drifts toward the
/// next one, so changes are gradual.
#[derive(Clone, Copy, Debug, Default)]
pub struct SeasonNow {
    pub temp: f32,
    pub swing: f32,
    pub sunrise: f32,
    pub sunset: f32,
    pub growth: f32,
    pub autumn: f32,
    pub leaf_drop: f32,
}

/// Light for drawing: 0 = full day, 1 = deep night; warm = dawn/dusk glow 0..1.
#[derive(Clone, Copy, Debug, Default)]
pub struct Light {
    pub dark: f32,
    pub warm: f32,
    /// True for dawn, false for dusk (picks the glow colour).
    pub dawn: bool,
}

pub struct Calendar {
    pub year: u64,
    pub season: usize,
    pub day_of_season: u32,
    /// Hour of day, 0..24 (fractional).
    pub hour: f32,
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn ramp(a: f32, b: f32, v: f32) -> f32 {
    ((v - a) / (b - a)).clamp(0.0, 1.0)
}

pub fn calendar(c: &ClimateDef, tick: u64) -> Calendar {
    let day = tick / DAY_TICKS;
    let frac = (tick % DAY_TICKS) as f32 / DAY_TICKS as f32;
    let dps = c.days_per_season.max(1) as u64;
    let n = c.seasons.len().max(1) as u64;
    Calendar {
        year: day / (dps * n) + 1,
        season: ((day / dps) % n) as usize,
        day_of_season: (day % dps) as u32 + 1,
        // The day counter starts at 06:00.
        hour: (6.0 + frac * 24.0) % 24.0,
    }
}

pub fn season_now(c: &ClimateDef, tick: u64) -> SeasonNow {
    if c.seasons.is_empty() {
        return SeasonNow { temp: 15.0, swing: 5.0, sunrise: 6.0, sunset: 19.0, growth: 1.0, ..Default::default() };
    }
    let n = c.seasons.len();
    let pos = tick as f64 / DAY_TICKS as f64 / c.days_per_season.max(1) as f64;
    let base = pos.floor();
    // Hold the season's own values for the first half, then blend.
    let f = (((pos - base) as f32 - 0.5) * 2.0).clamp(0.0, 1.0);
    let i = (base as i64).rem_euclid(n as i64) as usize;
    let (a, b) = (&c.seasons[i], &c.seasons[(i + 1) % n]);
    SeasonNow {
        temp: lerp(a.temp, b.temp, f),
        swing: lerp(a.swing, b.swing, f),
        sunrise: lerp(a.sunrise, b.sunrise, f),
        sunset: lerp(a.sunset, b.sunset, f),
        growth: lerp(a.growth, b.growth, f),
        autumn: lerp(a.autumn, b.autumn, f),
        leaf_drop: lerp(a.leaf_drop, b.leaf_drop, f),
    }
}

pub fn light(c: &ClimateDef, tick: u64, clouds: f32) -> Light {
    let s = season_now(c, tick);
    let h = calendar(c, tick).hour;
    let day = ramp(s.sunrise - 1.0, s.sunrise + 0.7, h) * (1.0 - ramp(s.sunset - 0.7, s.sunset + 1.0, h));
    let near = |x: f32| (1.0 - (h - x).abs() / 1.3).max(0.0);
    let (dw, dk) = (near(s.sunrise), near(s.sunset));
    Light {
        // Heavy cloud makes the day a bit dimmer too.
        dark: (1.0 - day) .max(clouds * 0.25),
        warm: dw.max(dk) * (1.0 - clouds * 0.7),
        dawn: dw >= dk,
    }
}

/// Words for a temperature, for the UI.
pub fn feels(t: f32) -> &'static str {
    match t {
        t if t < -10.0 => "Bitter cold",
        t if t < 0.0 => "Freezing",
        t if t < 8.0 => "Chilly",
        t if t < 16.0 => "Cool",
        t if t < 25.0 => "Mild",
        t if t < 32.0 => "Warm",
        _ => "Hot",
    }
}

impl Env {
    pub fn weather_def<'a>(&self, c: &'a ClimateDef) -> Option<&'a WeatherDef> {
        c.weather.iter().find(|w| w.id == self.weather)
    }

    /// Is precipitation falling as snow right now?
    pub fn snowing(&self) -> bool {
        self.precip > 0.05 && self.temp < 0.5
    }

    pub fn raining(&self) -> bool {
        self.precip > 0.05 && self.temp >= 0.5
    }

    /// Wind as a vector (toward which it blows) scaled by strength.
    pub fn wind_vec(&self) -> (f32, f32) {
        (self.wind_dir.cos() * self.wind, self.wind_dir.sin() * self.wind)
    }

    /// How fast plants grow now: the season's rate, slowed by cold.
    pub fn growth(&self, c: &ClimateDef, tick: u64) -> f32 {
        season_now(c, tick).growth * ramp(0.0, 8.0, self.temp)
    }

    /// Walking speed multiplier (deep snow is slow).
    pub fn walk_speed(&self) -> f32 {
        1.0 - 0.35 * self.snow
    }

    fn pick_weather(&mut self, c: &ClimateDef, tick: u64, rng: &mut Rng) {
        let cal = calendar(c, tick);
        let Some(season) = c.seasons.get(cal.season) else { return };
        let options: Vec<(&WeatherDef, f32)> = c
            .weather
            .iter()
            .filter_map(|w| season.weather.get(&w.id).map(|&k| (w, k.max(0.0))))
            .filter(|x| x.1 > 0.0)
            .collect();
        let total: f32 = options.iter().map(|o| o.1).sum();
        let Some(mut chosen) = options.first().map(|o| o.0) else { return };
        let mut r = rng.f32() * total;
        for (w, k) in &options {
            if r < *k {
                chosen = w;
                break;
            }
            r -= k;
        }
        self.weather = chosen.id.clone();
        let hours = rng.range_f32(chosen.hours.0, chosen.hours.1.max(chosen.hours.0));
        self.weather_until = tick + (hours * HOUR_TICKS as f32) as u64;
        self.wind_target = rng.range_f32(chosen.wind.0, chosen.wind.1.max(chosen.wind.0));
        self.wind_dir += rng.range_f32(-0.8, 0.8);
    }

    /// Advance one tick.
    pub fn tick(&mut self, c: &ClimateDef, tick: u64, rng: &mut Rng) {
        if self.weather.is_empty() || tick >= self.weather_until || self.weather_def(c).is_none() {
            self.pick_weather(c, tick, rng);
        }
        let w = self.weather_def(c).cloned();
        let (p, cl, fg, wt) = w.map(|w| (w.precipitation, w.clouds, w.fog, w.temp)).unwrap_or_default();
        // Ease toward the weather over roughly an hour.
        let k = 1.0 / HOUR_TICKS as f32;
        let ease = |v: &mut f32, target: f32| *v += (target - *v) * (k * 3.0).min(1.0);
        ease(&mut self.precip, p);
        ease(&mut self.clouds, cl);
        ease(&mut self.fog, fg);
        ease(&mut self.wind, self.wind_target);
        // Wind direction wanders slowly.
        if tick % 60 == 0 {
            self.wind_dir += rng.range_f32(-0.05, 0.05);
        }

        let s = season_now(c, tick);
        let h = calendar(c, tick).hour;
        // Coldest around 3:00, warmest around 15:00.
        let daily = ((h - 15.0) / 24.0 * std::f32::consts::TAU).cos();
        let target = s.temp + s.swing * daily + wt * self.precip.max(self.clouds).max(0.3) + self.biome_temp;
        ease(&mut self.temp, target);

        // Ground: rain soaks it, sun and wind dry it; snow piles up below freezing, melts above.
        if self.raining() {
            self.wetness = (self.wetness + self.precip * k * 2.0).min(1.0);
        } else {
            let dry = (0.3 + self.temp.max(0.0) / 20.0 + self.wind * 0.5) * k / 4.0;
            self.wetness = (self.wetness - dry).max(0.0);
        }
        if self.snowing() {
            self.snow = (self.snow + self.precip * k / 5.0).min(1.0);
        } else if self.temp > 0.5 {
            self.snow = (self.snow - self.temp * k / 30.0 - if self.raining() { k / 3.0 } else { 0.0 }).max(0.0);
        }
    }
}
