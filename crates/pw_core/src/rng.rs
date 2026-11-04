use serde::{Deserialize, Serialize};

pub fn splitmix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

pub fn hash2(seed: u64, a: i64, b: i64) -> u64 {
    splitmix64(seed ^ splitmix64((a as u64) ^ splitmix64(b as u64 ^ 0x51_7CC1_B727_220A)))
}

pub fn hash3(seed: u64, a: i64, b: i64, c: i64) -> u64 {
    hash2(splitmix64(seed ^ c as u64), a, b)
}

/// Stable string hash (FNV-1a), used to turn ids into seeds.
pub fn hash_str(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Map a hash to [0, 1).
pub fn unit(h: u64) -> f32 {
    (h >> 40) as f32 / (1u64 << 24) as f32
}

/// Small deterministic RNG (splitmix64 stream).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self { state: splitmix64(seed) }
    }
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        splitmix64(self.state)
    }
    /// Uniform in [0, 1).
    pub fn f32(&mut self) -> f32 {
        unit(self.next_u64())
    }
    pub fn range_f32(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.f32()
    }
    /// Inclusive range.
    pub fn range_i32(&mut self, lo: i32, hi: i32) -> i32 {
        if hi <= lo {
            return lo;
        }
        lo + (self.next_u64() % ((hi - lo + 1) as u64)) as i32
    }
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next_u64() % n as u64) as usize }
    }
    pub fn chance(&mut self, p: f32) -> bool {
        self.f32() < p
    }
}
