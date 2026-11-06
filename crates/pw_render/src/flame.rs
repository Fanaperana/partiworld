//! Animated flames in campfires (objects tagged `flame`): a small loop of
//! generated pixel-art frames sized for the top-down view, layered from a deep red rim through orange and
//! yellow to a pale core, flickering upward and throwing sparks.

use bevy::prelude::*;

use pw_core::{splitmix64, unit};

use crate::{GameRes, Views, new_image};

pub const W: u32 = 9;
pub const H: u32 = 10;
const FRAMES: usize = 12;

#[derive(Component)]
pub struct FlameSprite;

/// Smooth noise that loops every `FRAMES` frames as it rises.
fn noise(x: f32, y: f32, seed: u64) -> f32 {
    let (xi, yi) = (x.floor() as i64, y.floor() as i64);
    let (fx, fy) = (x - x.floor(), y - y.floor());
    let period = (FRAMES * 2) as i64;
    let v = |a: i64, b: i64| unit(splitmix64(seed ^ (a as u64).wrapping_mul(0x9E37) ^ (b.rem_euclid(period) as u64).wrapping_mul(0x85EB_CA6B)));
    let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let top = v(xi, yi) + (v(xi + 1, yi) - v(xi, yi)) * sx;
    let bot = v(xi, yi + 1) + (v(xi + 1, yi + 1) - v(xi, yi + 1)) * sx;
    top + (bot - top) * sy
}

/// One frame, RGBA, top row first: a rounded body with a pale core, an
/// orange and yellow middle and deep red tongues that sway and climb.
fn frame(f: usize) -> Vec<u8> {
    let mut px = vec![0u8; (W * H * 4) as usize];
    let cx = (W as f32 - 1.0) / 2.0;
    let t = f as f32;
    let colors: [(f32, [u8; 4]); 5] = [
        (0.95, [246, 232, 196, 255]),
        (0.72, [244, 198, 110, 255]),
        (0.5, [234, 132, 60, 255]),
        (0.3, [186, 64, 44, 255]),
        (0.14, [118, 36, 42, 230]),
    ];
    for y in 0..H {
        // y = 0 at the base of the flame.
        let up = y as f32 / H as f32;
        let sway = (t * std::f32::consts::TAU / FRAMES as f32 + up * 4.0).sin() * 0.8 * up * up;
        let half = 3.9 * (1.0 - up).powf(0.85) * (0.55 + 0.45 * (up / 0.18).min(1.0)) + 0.3;
        for x in 0..W {
            let r = (x as f32 - cx - sway).abs() / half;
            // Noise climbs two cells a frame and loops with the animation.
            let n = noise(x as f32 * 0.6, y as f32 * 0.5 - t * 2.0, 0xF1A3) * 0.6 + noise(x as f32 * 1.2, y as f32 * 0.9 - t * 2.0, 0x5EED) * 0.4;
            let heat = 1.25 - r * r * 0.95 - up * 1.05 - n * 0.55 * (0.4 + up);
            if let Some((_, c)) = colors.iter().find(|(th, _)| heat > *th) {
                let i = (((H - 1 - y) * W + x) * 4) as usize;
                px[i..i + 4].copy_from_slice(c);
            }
        }
    }
    // Sparks: a few bright specks drifting up through the loop.
    for s in 0..2u64 {
        let h = splitmix64(s * 7919 + 11);
        let life = ((t / FRAMES as f32) + unit(h)) % 1.0;
        let y = (H as f32 * (0.5 + life * 0.5)) as u32;
        let x = (cx + (unit(h ^ 0x77) - 0.5) * 5.0 + (life * 9.0 + s as f32).sin() * 1.2).round() as i32;
        let i = (((H - 1 - y.min(H - 1)) * W) as usize + x.max(0) as usize) * 4;
        if (0..W as i32).contains(&x) && y < H && life < 0.85 && px[i + 3] == 0 {
            px[i..i + 4].copy_from_slice(&[255, 170, 70, 255]);
        }
    }
    px
}

/// Keep a flame sprite on every finished `flame` object, and animate them.
pub fn sync_flames(
    mut commands: Commands,
    game: Res<GameRes>,
    mut views: ResMut<Views>,
    mut images: ResMut<Assets<Image>>,
    mut q: Query<(&mut Sprite, &mut Transform), With<FlameSprite>>,
    mut scan: Local<u32>,
) {
    if views.flame_frames.is_empty() {
        views.flame_frames = (0..FRAMES)
            .map(|f| {
                let mut img = new_image(W, H);
                img.data = Some(frame(f));
                images.add(img)
            })
            .collect();
    }
    let g = &game.0;
    // Find campfires now and then; animate every frame.
    *scan += 1;
    if *scan % 20 == 1 {
        let mut live = std::collections::HashSet::new();
        // Campfires still being built don't burn yet.
        let building: std::collections::HashSet<u32> = std::iter::once(&g.colony)
            .chain(g.tribes.list.iter().map(|t| &t.colony))
            .flat_map(|c| c.blueprints.values().filter_map(|b| b.obj))
            .collect();
        for (id, inst) in &g.world.objects.instances {
            if building.contains(id) || !g.content.protos.get(inst.def).is_some_and(|p| p.has_tag("flame")) {
                continue;
            }
            live.insert(*id);
            if !views.flames.contains_key(id) {
                let x = (inst.min.0 + inst.max.0) as f32 / 2.0 + 0.5;
                let y = (inst.min.1 + inst.max.1) as f32 / 2.0 + 0.5;
                // Base of the flame on the embers. Drawn above the night layer:
                // fire is bright in the dark.
                let t = Transform::from_xyz(x, y + H as f32 / 2.0 - 1.5, 31.0);
                let e = commands.spawn((Sprite::from_image(views.flame_frames[0].clone()), t, FlameSprite)).id();
                views.flames.insert(*id, e);
            }
        }
        let gone: Vec<_> = views.flames.keys().filter(|k| !live.contains(k)).copied().collect();
        for id in gone {
            if let Some(e) = views.flames.remove(&id) {
                commands.entity(e).despawn();
            }
        }
    }
    let anim = views.anim;
    for (id, e) in &views.flames {
        if let Ok((mut s, _)) = q.get_mut(*e) {
            // Each fire on its own beat, about 11 frames a second.
            let f = ((anim * 11.0) as usize + (*id as usize % 5) * 3) % FRAMES;
            if s.image != views.flame_frames[f] {
                s.image = views.flame_frames[f].clone();
            }
        }
    }
}
