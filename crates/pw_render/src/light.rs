//! Sunlight and darkness.
//!
//! The sun rises in the east and sets in the west: morning shadows point
//! west, noon shadows are short and point north, evening shadows point east.
//! Every particle has a height (canopies and roofs are tall, walls and rocks
//! less, bushes low), and the ground is in shadow when something between it
//! and the sun is tall enough. Night is a deep blue darkness per chunk, lifted
//! around fire, embers and lava, which light their surroundings.
//!
//! Both are computed on the CPU per chunk and spread over frames.

use std::f32::consts::PI;

use bevy::prelude::Vec2;
use pw_core::*;
use pw_game::Game;
use pw_world::*;

/// Heights in particles. A height `h` throws a shadow `h * len` long.
const H_CANOPY: u8 = 10;
const H_WALL: u8 = 6;
const H_BUSH: u8 = 3;
const H_LOW: u8 = 2;
/// Cells around a chunk read when casting shadows into it.
pub const SHADOW_MARGIN: i32 = 40;
/// Longest shadow, per unit of height (low sun).
const MAX_LEN: f32 = 4.0;
/// Night light map: one pixel per this many particles (smoothed when drawn).
pub const LIGHT_CELL: i32 = 4;
/// How far a fully glowing tile lights, in particles.
const LIGHT_RADIUS: f32 = 56.0;

/// Where the sun is now.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sun {
    /// Unit vector the shadows point along (world space: +x east, +y north).
    pub dir: Vec2,
    /// Shadow length per particle of height.
    pub len: f32,
    /// How dark shadows are, 0 (no sun) .. 1 (clear midday).
    pub strength: f32,
    /// Changes when shadows need redrawing (sun moved enough, clouds).
    pub key: u32,
}

/// The sun's position and shadow strength for this moment.
pub fn sun(g: &Game) -> Sun {
    let c = &g.content.climate;
    let s = pw_game::env::season_now(c, g.tick);
    let h = pw_game::env::calendar(c, g.tick).hour;
    let day = (s.sunset - s.sunrise).max(1.0);
    let f = (h - s.sunrise) / day;
    if !(0.0..=1.0).contains(&f) {
        return Sun { dir: Vec2::X, len: 0.0, strength: 0.0, key: 0 };
    }
    let a = f * PI;
    // Long days have a high sun.
    let top = (35.0 + (day - 12.0) * 4.0).clamp(20.0, 65.0).to_radians();
    let elev = (a.sin() * top).max(4f32.to_radians());
    // Toward the sun: east at dawn, a little south at noon, west at dusk.
    let toward = Vec2::new(a.cos(), -0.45 * a.sin()).normalize_or(Vec2::X);
    let len = (1.0 / elev.tan()).min(MAX_LEN);
    // Fades in after sunrise and out before sunset; clouds blur it away.
    let edge = (a.sin() / 0.2).min(1.0);
    let strength = edge * (1.0 - g.env.clouds * 0.85).max(0.0) * (1.0 - g.env.fog * 0.8);
    // About every 10 in-game minutes, or when the sky changes.
    let key = 1 + (f * 144.0) as u32 * 16 + (strength * 15.0).round() as u32;
    Sun { dir: -toward, len, strength, key }
}

/// Height per material, per layer (floor, solid, overlay).
pub fn heights(content: &Content, leafless: bool) -> Vec<[u8; 3]> {
    (0..content.mats.len())
        .map(|i| {
            let m = i as MatId;
            if m == NONE_MAT {
                return [0, 0, 0];
            }
            let rt = content.mat_rt(m);
            let solid = if rt.blocks {
                H_WALL
            } else if rt.leaves || rt.fruit {
                H_BUSH
            } else {
                H_LOW
            };
            let bare = rt.leaves && !rt.evergreen && leafless;
            let over = if rt.color[3] > 100 && !bare { H_CANOPY } else { 0 };
            [0, solid, over]
        })
        .collect()
}

/// Light each material gives at night, 0..1 (tag `glow`).
pub fn glows(content: &Content) -> Vec<f32> {
    (0..content.mats.len()).map(|i| if content.mat_rt(i as MatId).has_tag("glow") { 1.0 } else { 0.0 }).collect()
}

/// Heights of a chunk plus a margin, read from it and its neighbours.
/// Cells in chunks that aren't loaded count as flat.
fn height_grid(world: &World, cp: ChunkPos, heights: &[[u8; 3]]) -> Vec<u8> {
    let side = CHUNK + 2 * SHADOW_MARGIN;
    let mut grid = vec![0u8; (side * side) as usize];
    let (ox, oy) = cp.origin();
    let (gx0, gy0) = (ox - SHADOW_MARGIN, oy - SHADOW_MARGIN);
    for ny in -1..=1 {
        for nx in -1..=1 {
            let np = ChunkPos::new(cp.x + nx, cp.y + ny);
            let Some(ch) = world.chunks.get(&np) else { continue };
            let (cx, cy) = np.origin();
            // Overlap of this chunk with the grid, in world coordinates.
            let (x0, x1) = (cx.max(gx0), (cx + CHUNK).min(gx0 + side));
            let (y0, y1) = (cy.max(gy0), (cy + CHUNK).min(gy0 + side));
            for y in y0..y1 {
                let row = ((y - cy) * CHUNK) as usize;
                let grow = ((y - gy0) * side) as usize;
                for x in x0..x1 {
                    let i = row + (x - cx) as usize;
                    let h = heights[ch.mat[1][i] as usize][1].max(heights[ch.mat[2][i] as usize][2]);
                    grid[grow + (x - gx0) as usize] = h;
                }
            }
        }
    }
    grid
}

/// Paint a chunk's shadow image (RGBA, top row first): a dark tint where
/// something stands between the ground and the sun.
pub fn paint_shadows(world: &World, cp: ChunkPos, sun: &Sun, heights: &[[u8; 3]], out: &mut [u8]) {
    out.fill(0);
    if sun.strength <= 0.01 || sun.len <= 0.0 {
        return;
    }
    let grid = height_grid(world, cp, heights);
    let side = CHUNK + 2 * SHADOW_MARGIN;
    // Steps from a cell toward the sun, with the height needed at each.
    let reach = ((H_CANOPY as f32 * sun.len).ceil() as i32).min(SHADOW_MARGIN);
    let toward = -sun.dir;
    let steps: Vec<(isize, f32)> = (1..=reach)
        .map(|k| {
            let (dx, dy) = ((toward.x * k as f32).round() as i32, (toward.y * k as f32).round() as i32);
            ((dy * side + dx) as isize, k as f32 / sun.len)
        })
        .collect();
    let alpha = sun.strength * 120.0;
    for ly in 0..CHUNK {
        let row = (CHUNK - 1 - ly) as usize;
        for lx in 0..CHUNK {
            let gi = ((ly + SHADOW_MARGIN) * side + lx + SHADOW_MARGIN) as isize;
            let here = grid[gi as usize] as f32;
            if here >= H_CANOPY as f32 {
                continue; // tops of canopies and roofs are in the sun
            }
            // The nearest caster decides; shadows soften toward their tips.
            let hit = steps.iter().position(|&(off, need)| grid[(gi + off) as usize] as f32 - here >= need);
            if let Some(k) = hit {
                let fade = 1.0 - 0.45 * k as f32 / steps.len().max(1) as f32;
                let p = (row * CHUNK as usize + lx as usize) * 4;
                out[p..p + 4].copy_from_slice(&[14, 18, 38, (alpha * fade) as u8]);
            }
        }
    }
}

/// Glow per tile around a chunk (tiles of `TILE` particles), for night light.
fn glow_tiles(world: &World, cp: ChunkPos, glows: &[f32], margin_tiles: i32) -> Vec<(f32, f32, f32)> {
    let mut out = Vec::new();
    let (ox, oy) = cp.origin();
    let span = CHUNK / TILE;
    for ty in -margin_tiles..span + margin_tiles {
        for tx in -margin_tiles..span + margin_tiles {
            let t = tile_of(ox + tx * TILE, oy + ty * TILE);
            let Some(ch) = world.chunk(t.chunk()) else { continue };
            let (cx, cy) = t.chunk().origin();
            let (x0, y0) = t.origin();
            let mut g = 0.0;
            for py in (0..TILE).step_by(2) {
                for px in (0..TILE).step_by(2) {
                    let i = ((y0 + py - cy) * CHUNK + (x0 + px - cx)) as usize;
                    g += glows[ch.mat[0][i] as usize].max(glows[ch.mat[1][i] as usize]);
                }
            }
            if g > 0.0 {
                let (x, y) = t.center();
                out.push((x, y, (g / 16.0).min(1.0)));
            }
        }
    }
    out
}

/// Pixels per side of a night image: the chunk plus a one-pixel border shared
/// with the neighbours, so smoothing runs across chunk edges without a seam.
pub fn night_size() -> i32 {
    CHUNK / LIGHT_CELL + 2
}

/// Paint a chunk's night layer (RGBA, top row first, `LIGHT_CELL` particles a
/// pixel, with a one-pixel border): darkness `dark` everywhere, lifted and
/// warmed near glowing tiles.
pub fn paint_night(world: &World, cp: ChunkPos, dark: f32, glows: &[f32], out: &mut [u8]) {
    let n = night_size();
    let margin = (LIGHT_RADIUS as i32 / TILE) + 2;
    let lights = glow_tiles(world, cp, glows, margin);
    let (ox, oy) = cp.origin();
    let (ox, oy) = (ox - LIGHT_CELL, oy - LIGHT_CELL);
    let night = [6.0, 9.0, 26.0];
    let warm = [255.0, 160.0, 70.0];
    for py in 0..n {
        let row = (n - 1 - py) as usize;
        let y = (oy + py * LIGHT_CELL) as f32 + LIGHT_CELL as f32 / 2.0;
        for px in 0..n {
            let x = (ox + px * LIGHT_CELL) as f32 + LIGHT_CELL as f32 / 2.0;
            let mut l = 0.0f32;
            for &(lx, ly, g) in &lights {
                let r = LIGHT_RADIUS * (0.5 + 0.5 * g.sqrt());
                let d2 = (lx - x).powi(2) + (ly - y).powi(2);
                if d2 < r * r {
                    let f = 1.0 - d2.sqrt() / r;
                    l += f * f * (0.6 + 0.4 * g);
                }
            }
            let l = l.min(1.0);
            let a = dark * (1.0 - l * 0.92);
            // Lit areas take a warm tint instead of the night blue.
            let t = (l * 1.4).min(1.0);
            let c = [0, 1, 2].map(|k| night[k] + (warm[k] - night[k]) * t);
            let alpha = (a + l * 0.16 * dark).min(1.0);
            let p = (row * n as usize + px as usize) * 4;
            out[p..p + 4].copy_from_slice(&[c[0] as u8, c[1] as u8, c[2] as u8, (alpha * 255.0) as u8]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Light from a fire crosses into the next chunk without a seam.
    #[test]
    fn fire_light_crosses_chunk_borders() {
        let mut world = World::new(1);
        for cp in [ChunkPos::new(-1, -1), ChunkPos::new(0, -1)] {
            world.insert_chunk(Chunk::new(cp));
        }
        // A glowing 4x4 patch at x 18..22, y -30..-26 (in chunk 0,-1, near its left edge).
        let (cx, cy) = ChunkPos::new(0, -1).origin();
        for y in -30..-26 {
            for x in 18..22 {
                let ch = world.chunks.get_mut(&ChunkPos::new(0, -1)).unwrap();
                ch.mat[1][((y - cy) * CHUNK + (x - cx)) as usize] = 1;
            }
        }
        let glows = vec![0.0, 1.0];
        let n = night_size() as usize;
        let alpha_at = |cp: ChunkPos, x: i32, y: i32| {
            let mut out = vec![0u8; n * n * 4];
            paint_night(&world, cp, 0.9, &glows, &mut out);
            let (ox, oy) = cp.origin();
            let (px, py) = (((x - ox) / LIGHT_CELL + 1) as usize, ((y - oy) / LIGHT_CELL + 1) as usize);
            out[((n - 1 - py) * n + px) * 4 + 3]
        };
        let right = alpha_at(ChunkPos::new(0, -1), 2, -28);
        let left = alpha_at(ChunkPos::new(-1, -1), -3, -28);
        let far = alpha_at(ChunkPos::new(-1, -1), -200, -28);
        println!("right {right} left {left} far {far}");
        assert!((right as i32 - left as i32).abs() < 25, "seam: {right} vs {left}");
        assert!(left < far, "the next chunk should be lit near the fire");
    }
}
