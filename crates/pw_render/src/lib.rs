//! Bevy front end for drawing: one texture per chunk for ground + solids,
//! one for the overlay (canopies, roofs) drawn above colonists, plus sprites
//! for colonists, items and blueprints. All images are generated from
//! particle data; there are no sprite files.

use std::collections::{HashMap, HashSet};

use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::window::PrimaryWindow;
use pw_core::*;
use pw_game::{Game, TPS};
use pw_world::*;

/// The running simulation.
#[derive(Resource)]
pub struct GameRes(pub Game);

#[derive(Resource)]
pub struct RenderSettings {
    pub show_overlay: bool,
    /// Fade roofs and canopies around colonists under them and around the cursor.
    pub see_through: bool,
    /// World position of the mouse (set by the UI), for the see-through circle.
    pub cursor: Option<Vec2>,
    pub overlay_alpha: f32,
    pub show_zones: bool,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            show_overlay: true,
            see_through: true,
            cursor: None,
            overlay_alpha: 1.0,
            show_zones: true,
        }
    }
}

/// 0 = paused, 1..=3 = speed.
#[derive(Resource)]
pub struct SimClock {
    pub speed: u8,
    acc: f32,
    pub last_ticks: u32,
    /// Colonist positions before the latest tick, so drawing can glide
    /// between ticks instead of jumping (the sim runs at a different rate
    /// than the screen).
    /// Keyed by `pawn_key` (owner + pawn id).
    prev: HashMap<u64, (f32, f32)>,
    prev_life: HashMap<u32, (f32, f32)>,
}

/// One key for any person: owner 0 is the player's colony, else a tribe id.
pub fn pawn_key(owner: u32, id: u32) -> u64 {
    ((owner as u64) << 32) | id as u64
}

impl SimClock {
    /// Where to draw a colonist this frame: between its last two tick positions.
    pub fn draw_pos(&self, p: &pw_colony::Pawn) -> Vec2 {
        self.glide(self.prev.get(&pawn_key(0, p.id)), p.pos)
    }

    /// Same for anyone's people (`owner` as in `Game::all_pawns`).
    pub fn draw_pos_of(&self, owner: u32, p: &pw_colony::Pawn) -> Vec2 {
        self.glide(self.prev.get(&pawn_key(owner, p.id)), p.pos)
    }

    /// Same for a wild creature.
    pub fn draw_pos_life(&self, c: &pw_life::Creature) -> Vec2 {
        self.glide(self.prev_life.get(&c.id), c.pos)
    }

    fn glide(&self, prev: Option<&(f32, f32)>, pos: (f32, f32)) -> Vec2 {
        let now = Vec2::new(pos.0, pos.1);
        let Some(&(x, y)) = prev else { return now };
        let before = Vec2::new(x, y);
        // A jump (step out of a wall, load) is not interpolated.
        if before.distance(now) > TILE as f32 * 2.0 || self.speed == 0 {
            return now;
        }
        before.lerp(now, self.acc.clamp(0.0, 1.0))
    }
}

impl Default for SimClock {
    fn default() -> Self {
        Self {
            speed: 1,
            acc: 0.0,
            last_ticks: 0,
            prev: HashMap::new(),
            prev_life: HashMap::new(),
        }
    }
}

impl SimClock {
    pub fn multiplier(&self) -> f32 {
        match self.speed {
            0 => 0.0,
            1 => 1.0,
            2 => 3.0,
            _ => 6.0,
        }
    }
}

#[derive(Component)]
pub struct MainCamera;

/// Most chunks the view may span (width, height). Every visible chunk is
/// fully loaded and drawn, so this bounds memory and load time when zoomed out.
pub const MAX_VIEW_CHUNKS: (f32, f32) = (16.0, 10.0);

/// Furthest zoom-out (orthographic scale) for this window size.
pub fn max_zoom(window_w: f32, window_h: f32) -> f32 {
    let w = MAX_VIEW_CHUNKS.0 * CHUNK as f32 / window_w.max(1.0);
    let h = MAX_VIEW_CHUNKS.1 * CHUNK as f32 / window_h.max(1.0);
    w.min(h).max(0.1)
}

#[derive(Component)]
struct ChunkLayer;
#[derive(Component)]
struct PawnSprite;
#[derive(Component)]
struct CarrySprite;
#[derive(Component)]
struct ItemSprite;
#[derive(Component)]
struct BlueprintSprite;
#[derive(Component)]
struct ShadowSprite;
#[derive(Component)]
struct AnimalSprite;
/// Full-view tints: sky light (night, dawn, dusk) and weather (cloud, fog).
#[derive(Component)]
struct NightTint;
#[derive(Component)]
struct FogTint;

struct ChunkView {
    base: Entity,
    over: Entity,
    base_img: Handle<Image>,
    over_img: Handle<Image>,
}

#[derive(Resource, Default)]
struct Views {
    chunks: HashMap<ChunkPos, ChunkView>,
    pawns: HashMap<u64, (Entity, Entity, Entity)>,
    shadow_img: Option<Handle<Image>>,
    /// Wild creature id -> (sprite entity, image key).
    animals: HashMap<u32, (Entity, (usize, u8, u8))>,
    animal_images: HashMap<(usize, u8, u8), Handle<Image>>,
    items: HashMap<u64, (Entity, usize, TilePos)>,
    blueprints: HashMap<u32, Entity>,
    item_images: HashMap<usize, Handle<Image>>,
    bp_images: HashMap<(usize, u64), (Handle<Image>, i32, i32, u32, u32)>,
    /// See-through circles drawn last frame: (world x, world y, radius).
    holes: Vec<(f32, f32, f32)>,
    look: Look,
    /// Bumped when content changes so cached sprites rebuild.
    content_ptr: usize,
}

pub struct PwRenderPlugin;

impl Plugin for PwRenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RenderSettings>()
            .init_resource::<SimClock>()
            .init_resource::<Views>()
            .insert_resource(ClearColor(Color::srgb(0.06, 0.06, 0.08)))
            .add_systems(Startup, setup_camera)
            .add_systems(
                Update,
                (
                    run_sim,
                    update_view,
                    sync_chunks,
                    sync_items,
                    sync_blueprints,
                    sync_pawns,
                    sync_animals,
                    day_night,
                    weather_fx,
                    draw_flashes,
                    draw_zones,
                )
                    .chain(),
            );
    }
}

fn setup_camera(mut commands: Commands, game: Res<GameRes>) {
    let (x, y) = game.0.spawn_tile().center();
    commands.spawn((
        Camera2d,
        MainCamera,
        Projection::Orthographic(OrthographicProjection {
            scale: 0.25,
            ..OrthographicProjection::default_2d()
        }),
        Transform::from_xyz(x, y, 100.0),
    ));
    commands.spawn((
        Sprite::from_color(Color::NONE, Vec2::ONE),
        Transform::from_xyz(x, y, 50.0),
        NightTint,
    ));
    commands.spawn((
        Sprite::from_color(Color::NONE, Vec2::ONE),
        Transform::from_xyz(x, y, 51.0),
        FogTint,
    ));
}

/// The sky tint for this moment: night blue, dawn pink-orange, dusk orange-violet.
fn sky_color(g: &Game) -> Color {
    let l = g.light();
    let night = Vec3::new(0.03, 0.05, 0.16);
    let glow = if l.dawn {
        Vec3::new(1.0, 0.62, 0.45)
    } else {
        Vec3::new(0.95, 0.42, 0.28)
    };
    let w = l.warm / (l.warm + l.dark + 1e-3);
    let c = night.lerp(glow, w);
    let a = (0.6 * l.dark + 0.22 * l.warm).min(0.72);
    Color::srgba(c.x, c.y, c.z, a)
}

#[allow(clippy::type_complexity)]
fn day_night(
    game: Res<GameRes>,
    cam: Query<(&Transform, &Projection), (With<MainCamera>, Without<NightTint>, Without<FogTint>)>,
    window: Query<&Window, With<PrimaryWindow>>,
    mut tint: Query<(&mut Transform, &mut Sprite), (With<NightTint>, Without<FogTint>)>,
    mut fog: Query<(&mut Transform, &mut Sprite), (With<FogTint>, Without<NightTint>)>,
) {
    let (Ok((ct, p)), Ok(w)) = (cam.single(), window.single()) else {
        return;
    };
    let scale = match p {
        Projection::Orthographic(o) => o.scale,
        _ => 1.0,
    };
    let size = Vec2::new(w.width(), w.height()) * scale * 1.1;
    let g = &game.0;
    let fog_c = Color::srgba(
        0.78,
        0.8,
        0.84,
        (g.env.fog * 0.5 + g.env.clouds * 0.1).min(0.6),
    );
    for (q, c) in [(tint.single_mut(), sky_color(g)), (fog.single_mut(), fog_c)] {
        let Ok((mut t, mut s)) = q else { continue };
        t.translation.x = ct.translation.x;
        t.translation.y = ct.translation.y;
        if s.custom_size != Some(size) {
            s.custom_size = Some(size);
        }
        if s.color != c {
            s.color = c;
        }
    }
}

/// Falling rain and snow, drawn as short lines that drift with the wind.
fn weather_fx(
    game: Res<GameRes>,
    time: Res<Time>,
    cam: Query<(&Transform, &Projection), With<MainCamera>>,
    window: Query<&Window, With<PrimaryWindow>>,
    mut gizmos: Gizmos,
) {
    let env = &game.0.env;
    if env.precip < 0.03 {
        return;
    }
    let (Ok((ct, p)), Ok(w)) = (cam.single(), window.single()) else {
        return;
    };
    let scale = match p {
        Projection::Orthographic(o) => o.scale,
        _ => 1.0,
    };
    let view = Vec2::new(w.width(), w.height()) * scale;
    let origin = ct.translation.truncate() - view / 2.0;
    let snow = env.snowing();
    let (wx, wy) = env.wind_vec();
    // Screen-space speeds (pixels per second), turned into world units.
    let fall = if snow { 70.0 } else { 520.0 };
    let push = if snow { 160.0 } else { 260.0 };
    let vel = Vec2::new(wx * push, wy * push * 0.5 - fall) * scale;
    let n = (env.precip * if snow { 380.0 } else { 520.0 }) as u32;
    let t = time.elapsed_secs();
    let color = if snow {
        Color::srgba(1.0, 1.0, 1.0, 0.85)
    } else {
        Color::srgba(0.72, 0.8, 0.95, 0.45)
    };
    let streak = if snow {
        vel.normalize_or_zero() * 1.6 * scale
    } else {
        vel * 0.035
    };
    for i in 0..n {
        let h = splitmix64(i as u64 * 7919 + 13);
        let (u, v) = (unit(h), unit(splitmix64(h ^ 0x51)));
        let speed = 0.7 + unit(splitmix64(h ^ 0x52)) * 0.6;
        // Each drop loops through the view; offset by its own phase.
        let wobble = if snow {
            (t * 1.7 + u * 20.0).sin() * 6.0 * scale
        } else {
            0.0
        };
        let mut pos = Vec2::new(u * view.x, v * view.y) + vel * speed * t + Vec2::new(wobble, 0.0);
        pos.x = pos.x.rem_euclid(view.x);
        pos.y = pos.y.rem_euclid(view.y);
        let a = origin + pos;
        gizmos.line_2d(a, a + streak, color);
    }
}

/// Advance the simulation with a fixed tick rate scaled by the speed setting.
fn run_sim(time: Res<Time>, mut clock: ResMut<SimClock>, mut game: ResMut<GameRes>) {
    let m = clock.multiplier();
    clock.acc += time.delta_secs() * TPS * m;
    let mut n = 0;
    while clock.acc >= 1.0 && n < 24 {
        clock.prev = game
            .0
            .all_pawns()
            .map(|(o, p)| (pawn_key(o, p.id), p.pos))
            .collect();
        clock.prev_life = game
            .0
            .life
            .creatures
            .iter()
            .map(|c| (c.id, c.pos))
            .collect();
        game.0.step();
        clock.acc -= 1.0;
        n += 1;
    }
    if clock.acc > 4.0 {
        clock.acc = 0.0; // falling behind; drop time rather than spiral
    }
    clock.last_ticks = n;
}

/// Tell the game which chunks the camera sees, then stream.
fn update_view(
    mut game: ResMut<GameRes>,
    cam: Query<(&Transform, &Projection), With<MainCamera>>,
    window: Query<&Window, With<PrimaryWindow>>,
) {
    let (Ok((t, p)), Ok(w)) = (cam.single(), window.single()) else {
        return;
    };
    let scale = match p {
        Projection::Orthographic(o) => o.scale,
        _ => 1.0,
    };
    let half = Vec2::new(w.width(), w.height()) * 0.5 * scale;
    let min = t.translation.truncate() - half;
    let max = t.translation.truncate() + half;
    game.0.view = Some((
        chunk_of(min.x as i32, min.y as i32),
        chunk_of(max.x as i32, max.y as i32),
    ));
    game.0.stream_chunks();
}

fn new_image(w: u32, h: u32) -> Image {
    let mut img = Image::new_fill(
        Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[0, 0, 0, 0],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    img.sampler = ImageSampler::nearest();
    img
}

fn shade(c: [u8; 4], f: f32) -> [u8; 4] {
    [
        (c[0] as f32 * f).clamp(0.0, 255.0) as u8,
        (c[1] as f32 * f).clamp(0.0, 255.0) as u8,
        (c[2] as f32 * f).clamp(0.0, 255.0) as u8,
        c[3],
    ]
}

/// Cell size of the soft light/dark mottling on top of per-particle grain.
const MOTTLE: i32 = 6;

/// Smooth 0..1 value noise on a MOTTLE grid.
fn mottle(seed: u64, x: i32, y: i32) -> f32 {
    let (gx, gy) = (x.div_euclid(MOTTLE), y.div_euclid(MOTTLE));
    let fx = x.rem_euclid(MOTTLE) as f32 / MOTTLE as f32;
    let fy = y.rem_euclid(MOTTLE) as f32 / MOTTLE as f32;
    let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let v = |a: i32, b: i32| unit(hash2(seed, a as i64, b as i64));
    let top = v(gx, gy) + (v(gx + 1, gy) - v(gx, gy)) * sx;
    let bot = v(gx, gy + 1) + (v(gx + 1, gy + 1) - v(gx, gy + 1)) * sx;
    top + (bot - top) * sy
}

fn particle_color(content: &Content, m: MatId, seed: u64, x: i32, y: i32, layer: usize) -> [u8; 4] {
    let rt = content.mat_rt(m);
    // Mostly soft patches, a little per-particle grain: reads as texture, not static.
    let grain = unit(hash3(seed ^ 0xC0105, x as i64, y as i64, layer as i64));
    let soft = mottle(seed ^ 0x5EED ^ layer as u64, x, y);
    let h = soft * 0.6 + grain * 0.4;
    shade(rt.color, 1.0 + (h - 0.5) * rt.variance)
}

/// Seasonal look of the world, quantised so chunks only repaint when it
/// visibly changes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Look {
    /// Snow cover 0..8, autumn colour 0..10, leaves gone 0..10.
    snow: u8,
    autumn: u8,
    drop: u8,
}

impl Look {
    fn of(g: &Game) -> Self {
        let s = pw_game::env::season_now(&g.content.climate, g.tick);
        Look {
            snow: (g.env.snow * 8.0).round() as u8,
            autumn: (s.autumn * 10.0).round() as u8,
            drop: (s.leaf_drop * 10.0).round() as u8,
        }
    }
}

const SNOW: [u8; 4] = [236, 240, 247, 255];

fn mix(a: [u8; 4], b: [u8; 4], t: f32) -> [u8; 4] {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    [f(a[0], b[0]), f(a[1], b[1]), f(a[2], b[2]), a[3]]
}

/// Patchy cover: true where snow lies at this level (0..8).
fn snowy(seed: u64, x: i32, y: i32, level: u8) -> bool {
    if level == 0 {
        return false;
    }
    let n = mottle(seed ^ 0x5A0, x, y) * 0.7 + unit(hash2(seed ^ 0x5A1, x as i64, y as i64)) * 0.3;
    n < level as f32 / 8.0 * 1.15
}

/// Shading reaches this many cells from a changed particle.
const SHADE_REACH: i32 = 3;

type Rect = (i32, i32, i32, i32);

fn grow(r: Rect) -> Rect {
    let m = CHUNK - 1;
    (
        (r.0 - SHADE_REACH).max(0),
        (r.1 - SHADE_REACH).max(0),
        (r.2 + SHADE_REACH).min(m),
        (r.3 + SHADE_REACH).min(m),
    )
}

fn mat_at(v: &[MatId], lx: i32, ly: i32) -> MatId {
    if (0..CHUNK).contains(&lx) && (0..CHUNK).contains(&ly) {
        v[(ly * CHUNK + lx) as usize]
    } else {
        NONE_MAT
    }
}

/// Paint ground + solids for the cells in `r` (local, inclusive).
fn paint_base(chunk: &Chunk, content: &Content, seed: u64, r: Rect, base: &mut [u8], look: Look) {
    let (ox, oy) = chunk.pos.origin();
    let solid = &chunk.mat[1];
    let overlay = &chunk.mat[2];
    let blocks = |m: MatId| m != NONE_MAT && content.mat_rt(m).blocks;
    for ly in r.1..=r.3 {
        let row = (CHUNK - 1 - ly) as usize;
        for lx in r.0..=r.2 {
            let i = (ly * CHUNK + lx) as usize;
            let p = (row * CHUNK as usize + lx as usize) * 4;
            let (x, y) = (ox + lx, oy + ly);
            let s = solid[i];
            let col = if s != NONE_MAT {
                // Fake height: lit top edge, dark bottom edge.
                let mut c = particle_color(content, s, seed, x, y, 1);
                if blocks(s) && mat_at(solid, lx, ly - 1) == NONE_MAT {
                    c = shade(c, 0.62);
                } else if mat_at(solid, lx, ly + 1) == NONE_MAT {
                    c = shade(c, 1.18);
                }
                c
            } else {
                let fm = chunk.mat[0][i];
                let fr = content.mat_rt(fm);
                let mut c = particle_color(content, fm, seed, x, y, 0);
                if !fr.water && snowy(seed, x, y, look.snow) {
                    c = mix(c, SNOW, 0.9);
                }
                // Shadows from walls/rocks just below them and from canopies offset down-right.
                if blocks(mat_at(solid, lx, ly + 1)) {
                    c = shade(c, 0.6);
                } else if blocks(mat_at(solid, lx - 1, ly + 2)) {
                    c = shade(c, 0.8);
                } else {
                    let o = mat_at(overlay, lx - 3, ly + 3);
                    if o != NONE_MAT && content.mat_rt(o).color[3] > 128 {
                        c = shade(c, 0.78);
                    }
                }
                c
            };
            base[p..p + 4].copy_from_slice(&[col[0], col[1], col[2], 255]);
        }
    }
}

/// Radius (particles) of the see-through circle around the cursor and colonists.
const HOLE_CURSOR: f32 = 18.0;
const HOLE_PAWN: f32 = 13.0;
/// Overlay opacity at the middle of a see-through circle.
const HOLE_ALPHA: f32 = 0.18;

/// How opaque the overlay stays at a point, given the see-through circles.
fn hole_factor(holes: &[(f32, f32, f32)], x: f32, y: f32) -> f32 {
    let mut f: f32 = 1.0;
    for &(hx, hy, r) in holes {
        let d = ((x - hx).powi(2) + (y - hy).powi(2)).sqrt();
        // Soft edge over the outer 5 particles.
        let t = ((d - (r - 5.0)) / 5.0).clamp(0.0, 1.0);
        f = f.min(HOLE_ALPHA + (1.0 - HOLE_ALPHA) * t);
    }
    f
}

/// Paint the overlay layer (canopies, roofs) for the cells in `r`.
fn paint_over(
    chunk: &Chunk,
    content: &Content,
    seed: u64,
    r: Rect,
    over: &mut [u8],
    holes: &[(f32, f32, f32)],
    look: Look,
) {
    let (ox, oy) = chunk.pos.origin();
    let overlay = &chunk.mat[2];
    for ly in r.1..=r.3 {
        let row = (CHUNK - 1 - ly) as usize;
        for lx in r.0..=r.2 {
            let i = (ly * CHUNK + lx) as usize;
            let p = (row * CHUNK as usize + lx as usize) * 4;
            let o = overlay[i];
            if o == NONE_MAT {
                over[p..p + 4].copy_from_slice(&[0, 0, 0, 0]);
                continue;
            }
            let (x, y) = (ox + lx, oy + ly);
            let rt = content.mat_rt(o);
            let mut c = particle_color(content, o, seed, x, y, 2);
            if rt.leaves && !rt.evergreen {
                let h = unit(hash2(seed ^ 0x1EAF, x as i64, y as i64));
                // Leaves fall in patches, not evenly.
                let gone = mottle(seed ^ 0x1EA0, x, y) * 0.6 + h * 0.4;
                if gone < look.drop as f32 / 10.0 {
                    over[p..p + 4].copy_from_slice(&[0, 0, 0, 0]);
                    continue;
                }
                if look.autumn > 0 {
                    let tone = if h < 0.35 {
                        [196, 72, 38, 255]
                    } else if h < 0.75 {
                        [214, 128, 40, 255]
                    } else {
                        [226, 186, 64, 255]
                    };
                    c = mix(c, tone, look.autumn as f32 / 10.0 * 0.85);
                }
            }
            if (rt.roof || rt.leaves)
                && snowy(seed, x, y, if rt.roof { look.snow } else { look.snow / 2 })
            {
                c = mix(c, SNOW, 0.85);
            }
            let edge = [(1, 0), (-1, 0), (0, 1), (0, -1)]
                .iter()
                .any(|&(dx, dy)| mat_at(overlay, lx + dx, ly + dy) == NONE_MAT);
            if edge {
                c = shade(c, 0.8);
            } else if mat_at(overlay, lx + 1, ly + 1) == NONE_MAT {
                c = shade(c, 1.12);
            }
            if !holes.is_empty() {
                c[3] = (c[3] as f32 * hole_factor(holes, x as f32 + 0.5, y as f32 + 0.5)) as u8;
            }
            over[p..p + 4].copy_from_slice(&c);
        }
    }
}

fn sync_chunks(
    mut commands: Commands,
    mut game: ResMut<GameRes>,
    mut views: ResMut<Views>,
    mut images: ResMut<Assets<Image>>,
    settings: Res<RenderSettings>,
    mut vis: Query<&mut Visibility, With<ChunkLayer>>,
    mut sprites: Query<&mut Sprite, With<ChunkLayer>>,
) {
    let g = &mut game.0;
    let content = g.content.clone();
    let seed = g.seed;
    let holes = see_through_holes(g, &settings);
    let look = Look::of(g);
    if look != views.look {
        views.look = look;
        for ch in g.world.chunks.values_mut() {
            ch.mark_render_all();
        }
    }
    // Remove views for unloaded chunks.
    let gone: Vec<ChunkPos> = views
        .chunks
        .keys()
        .filter(|c| !g.world.chunks.contains_key(c))
        .copied()
        .collect();
    for c in gone {
        let v = views.chunks.remove(&c).unwrap();
        commands.entity(v.base).despawn();
        commands.entity(v.over).despawn();
        images.remove(&v.base_img);
        images.remove(&v.over_img);
    }
    let mut full_paints = 0;
    for (cp, chunk) in g.world.chunks.iter_mut() {
        if !views.chunks.contains_key(cp) {
            let base_img = images.add(new_image(CHUNK as u32, CHUNK as u32));
            let over_img = images.add(new_image(CHUNK as u32, CHUNK as u32));
            let (x, y) = cp.origin();
            let center = Vec2::new(x as f32 + CHUNK as f32 / 2.0, y as f32 + CHUNK as f32 / 2.0);
            let base = commands
                .spawn((
                    Sprite::from_image(base_img.clone()),
                    Transform::from_xyz(center.x, center.y, 0.0),
                    ChunkLayer,
                ))
                .id();
            let over = commands
                .spawn((
                    Sprite::from_image(over_img.clone()),
                    Transform::from_xyz(center.x, center.y, 20.0),
                    ChunkLayer,
                ))
                .id();
            views.chunks.insert(
                *cp,
                ChunkView {
                    base,
                    over,
                    base_img,
                    over_img,
                },
            );
            chunk.mark_render_all();
        }
        if !chunk.render_dirty {
            continue;
        }
        // Small changes (chopping, fire) redraw only the area around them and
        // are cheap; whole-chunk paints are budgeted per frame.
        let rect = match chunk.render_rect {
            Some(r) => grow(r),
            None if full_paints < 4 => (0, 0, CHUNK - 1, CHUNK - 1),
            None => continue,
        };
        if chunk.render_rect.is_none() {
            full_paints += 1;
        }
        let v = &views.chunks[cp];
        let (bh, oh) = (v.base_img.clone(), v.over_img.clone());
        if let Some(mut img) = images.get_mut(&bh) {
            if let Some(data) = img.data.as_mut() {
                paint_base(chunk, &content, seed, rect, data, look);
            }
        }
        if let Some(mut img) = images.get_mut(&oh) {
            if let Some(data) = img.data.as_mut() {
                paint_over(chunk, &content, seed, rect, data, &holes, look);
            }
        }
        chunk.render_dirty = false;
        chunk.render_rect = None;
    }
    // See-through circles moved: repaint the overlay where they were and are.
    if holes != views.holes {
        let old = std::mem::replace(&mut views.holes, holes.clone());
        for &(hx, hy, r) in old.iter().chain(holes.iter()) {
            let (x0, y0, x1, y1) = (
                (hx - r) as i32,
                (hy - r) as i32,
                (hx + r) as i32 + 1,
                (hy + r) as i32 + 1,
            );
            let (c0, c1) = (chunk_of(x0, y0), chunk_of(x1, y1));
            for cy in c0.y..=c1.y {
                for cx in c0.x..=c1.x {
                    let cp = ChunkPos::new(cx, cy);
                    let (Some(chunk), Some(v)) = (g.world.chunks.get(&cp), views.chunks.get(&cp))
                    else {
                        continue;
                    };
                    let (ox, oy) = cp.origin();
                    let m = CHUNK - 1;
                    let rect = (
                        (x0 - ox).clamp(0, m),
                        (y0 - oy).clamp(0, m),
                        (x1 - ox).clamp(0, m),
                        (y1 - oy).clamp(0, m),
                    );
                    if let Some(mut img) = images.get_mut(&v.over_img) {
                        if let Some(data) = img.data.as_mut() {
                            paint_over(chunk, &content, seed, rect, data, &holes, look);
                        }
                    }
                }
            }
        }
    }
    // Overlay visibility / fade.
    for v in views.chunks.values() {
        if let Ok(mut vis) = vis.get_mut(v.over) {
            vis.set_if_neq(if settings.show_overlay {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            });
        }
        if let Ok(mut s) = sprites.get_mut(v.over) {
            let c = Color::srgba(1.0, 1.0, 1.0, settings.overlay_alpha);
            if s.color != c {
                s.color = c;
            }
        }
    }
}

/// Build a sprite image. With `outline`, a 1-particle dark border is added
/// around the shape so it stands out on any ground.
/// Where roofs and canopies should turn see-through this frame: around the
/// cursor, and around every colonist standing under something.
fn see_through_holes(g: &Game, settings: &RenderSettings) -> Vec<(f32, f32, f32)> {
    if !settings.see_through || !settings.show_overlay {
        return Vec::new();
    }
    let covered = |x: f32, y: f32| {
        [(0, 0), (3, 0), (-3, 0), (0, 3), (0, -3)]
            .iter()
            .any(|&(dx, dy)| g.world.get(x as i32 + dx, y as i32 + dy, Layer::Overlay) != NONE_MAT)
    };
    let mut holes = Vec::new();
    if let Some(c) = settings.cursor {
        if covered(c.x, c.y) {
            holes.push((c.x.round(), c.y.round(), HOLE_CURSOR));
        }
    }
    for (_, p) in g.all_pawns() {
        if covered(p.pos.0, p.pos.1) {
            holes.push((p.pos.0.round(), p.pos.1.round(), HOLE_PAWN));
        }
    }
    holes
}

fn sprite_image(
    images: &mut Assets<Image>,
    s: &SpriteRt,
    slots: &std::collections::BTreeMap<String, String>,
    outline: bool,
) -> Handle<Image> {
    let (w, h) = (s.w.max(1) as usize, s.h.max(1) as usize);
    let mut px: Vec<[u8; 4]> = vec![[0; 4]; w * h];
    for (i, p) in s.pixels.iter().enumerate().take(w * h) {
        px[i] = *p;
    }
    for (i, slot) in &s.slots {
        if *i < px.len() {
            px[*i] = slots
                .get(slot)
                .and_then(|h| parse_color(h))
                .unwrap_or([255, 0, 255, 255]);
        }
    }
    let pad = if outline { 1 } else { 0 };
    let (ow, oh) = (w + pad * 2, h + pad * 2);
    let mut img = new_image(ow as u32, oh as u32);
    if let Some(data) = img.data.as_mut() {
        let solid = |x: i64, y: i64| {
            x >= 0
                && y >= 0
                && (x as usize) < w
                && (y as usize) < h
                && px[y as usize * w + x as usize][3] > 0
        };
        for oy in 0..oh {
            for ox in 0..ow {
                let (x, y) = (ox as i64 - pad as i64, oy as i64 - pad as i64);
                let c = if solid(x, y) {
                    px[y as usize * w + x as usize]
                } else if outline
                    && [(1, 0), (-1, 0), (0, 1), (0, -1)]
                        .iter()
                        .any(|&(dx, dy)| solid(x + dx, y + dy))
                {
                    [20, 16, 22, 220]
                } else {
                    [0, 0, 0, 0]
                };
                let i = (oy * ow + ox) * 4;
                data[i..i + 4].copy_from_slice(&c);
            }
        }
    }
    images.add(img)
}

/// Soft oval shadow under colonists.
fn shadow_image(images: &mut Assets<Image>) -> Handle<Image> {
    let (w, h) = (10u32, 5u32);
    let mut img = new_image(w, h);
    if let Some(data) = img.data.as_mut() {
        for y in 0..h {
            for x in 0..w {
                let dx = (x as f32 + 0.5 - w as f32 / 2.0) / (w as f32 / 2.0);
                let dy = (y as f32 + 0.5 - h as f32 / 2.0) / (h as f32 / 2.0);
                let d = dx * dx + dy * dy;
                let a = if d < 1.0 {
                    ((1.0 - d) * 140.0) as u8
                } else {
                    0
                };
                let i = ((y * w + x) * 4) as usize;
                data[i..i + 4].copy_from_slice(&[0, 0, 0, a]);
            }
        }
    }
    images.add(img)
}

fn item_image(
    views: &mut Views,
    images: &mut Assets<Image>,
    content: &Content,
    item: usize,
) -> Handle<Image> {
    views
        .item_images
        .entry(item)
        .or_insert_with(|| {
            sprite_image(
                images,
                &content.item_rt[item].sprite,
                &Default::default(),
                true,
            )
        })
        .clone()
}

fn check_content(views: &mut Views, content: &std::sync::Arc<Content>) {
    let ptr = std::sync::Arc::as_ptr(content) as usize;
    if views.content_ptr != ptr {
        views.content_ptr = ptr;
        views.item_images.clear();
        views.bp_images.clear();
        views.animal_images.clear();
    }
}

fn sync_items(
    mut commands: Commands,
    game: Res<GameRes>,
    mut views: ResMut<Views>,
    mut images: ResMut<Assets<Image>>,
) {
    let g = &game.0;
    check_content(&mut views, &g.content);
    let live: HashSet<u64> = g.all_items().map(|(o, s)| pawn_key(o, s.id)).collect();
    let gone: Vec<u64> = views
        .items
        .keys()
        .filter(|k| !live.contains(k))
        .copied()
        .collect();
    for id in gone {
        let (e, _, _) = views.items.remove(&id).unwrap();
        commands.entity(e).despawn();
    }
    for (owner, s) in g.all_items() {
        let key = pawn_key(owner, s.id);
        let (x, y) = s.tile.center();
        let pos = Transform::from_xyz(x, y, 3.0);
        match views.items.get(&key) {
            Some(&(_, item, tile)) if item == s.item && tile == s.tile => {}
            Some(&(e, item, _)) if item == s.item => {
                commands.entity(e).insert(pos);
                views.items.insert(key, (e, item, s.tile));
            }
            other => {
                if let Some(&(e, _, _)) = other {
                    commands.entity(e).despawn();
                }
                let img = item_image(&mut views, &mut images, &g.content, s.item);
                let e = commands
                    .spawn((Sprite::from_image(img), pos, ItemSprite))
                    .id();
                views.items.insert(key, (e, s.item, s.tile));
            }
        }
    }
}

fn sync_blueprints(
    mut commands: Commands,
    game: Res<GameRes>,
    mut views: ResMut<Views>,
    mut images: ResMut<Assets<Image>>,
) {
    let g = &game.0;
    let gone: Vec<u32> = views
        .blueprints
        .keys()
        .filter(|k| !g.colony.blueprints.contains_key(k))
        .copied()
        .collect();
    for id in gone {
        let e = views.blueprints.remove(&id).unwrap();
        commands.entity(e).despawn();
    }
    for b in g.colony.blueprints.values() {
        if views.blueprints.contains_key(&b.id) {
            continue;
        }
        let key = (b.def, b.seed);
        let content = &g.content;
        let img = views
            .bp_images
            .entry(key)
            .or_insert_with(|| {
                let cells = content.protos[b.def].cells(b.seed);
                let (minx, maxx) = cells
                    .iter()
                    .fold((i32::MAX, i32::MIN), |a, c| (a.0.min(c.dx), a.1.max(c.dx)));
                let (miny, maxy) = cells
                    .iter()
                    .fold((i32::MAX, i32::MIN), |a, c| (a.0.min(c.dy), a.1.max(c.dy)));
                let (w, h) = (
                    (maxx - minx + 1).max(1) as u32,
                    (maxy - miny + 1).max(1) as u32,
                );
                let mut img = new_image(w, h);
                if let Some(data) = img.data.as_mut() {
                    // See-through ghost: faint fill, bright edges.
                    let filled: HashSet<(i32, i32)> = cells.iter().map(|c| (c.dx, c.dy)).collect();
                    for c in &cells {
                        let col = content.mat_rt(c.mat).color;
                        let (px, py) = ((c.dx - minx) as u32, (maxy - c.dy) as u32);
                        let i = ((py * w + px) * 4) as usize;
                        let edge = [(1, 0), (-1, 0), (0, 1), (0, -1)]
                            .iter()
                            .any(|&(dx, dy)| !filled.contains(&(c.dx + dx, c.dy + dy)));
                        let ghost = if edge {
                            [150, 200, 255, 210]
                        } else {
                            [
                                (col[0] as f32 * 0.3 + 70.0) as u8,
                                (col[1] as f32 * 0.3 + 110.0) as u8,
                                235,
                                70,
                            ]
                        };
                        data[i..i + 4].copy_from_slice(&ghost);
                    }
                }
                (images.add(img), minx, miny, w, h)
            })
            .clone();
        let (handle, minx, miny, w, h) = img;
        let cx = b.origin.0 as f32 + minx as f32 + w as f32 / 2.0;
        let cy = b.origin.1 as f32 + miny as f32 + h as f32 / 2.0;
        let e = commands
            .spawn((
                Sprite::from_image(handle),
                Transform::from_xyz(cx, cy, 4.0),
                BlueprintSprite,
            ))
            .id();
        views.blueprints.insert(b.id, e);
    }
}

fn sync_pawns(
    mut commands: Commands,
    game: Res<GameRes>,
    mut views: ResMut<Views>,
    mut images: ResMut<Assets<Image>>,
    mut carry: Query<
        (&mut Sprite, &mut Visibility, &mut Transform),
        (With<CarrySprite>, Without<PawnSprite>),
    >,
    mut bodies: Query<
        &mut Transform,
        (
            With<PawnSprite>,
            Without<CarrySprite>,
            Without<ShadowSprite>,
        ),
    >,
    mut shadows: Query<
        &mut Transform,
        (
            With<ShadowSprite>,
            Without<CarrySprite>,
            Without<PawnSprite>,
        ),
    >,
    clock: Res<SimClock>,
) {
    let g = &game.0;
    let live: HashSet<u64> = g.all_pawns().map(|(o, p)| pawn_key(o, p.id)).collect();
    let gone: Vec<u64> = views
        .pawns
        .keys()
        .filter(|k| !live.contains(k))
        .copied()
        .collect();
    for id in gone {
        let (a, b, sh) = views.pawns.remove(&id).unwrap();
        commands.entity(a).despawn();
        commands.entity(b).despawn();
        commands.entity(sh).despawn();
    }
    for (owner, p) in g.all_pawns() {
        let key = pawn_key(owner, p.id);
        let d = clock.draw_pos_of(owner, p);
        let pos = Transform::from_xyz(d.x, d.y, 10.0);
        let carry_pos = Transform::from_xyz(d.x + 3.0, d.y + 4.0, 11.0);
        let shadow_pos = Transform::from_xyz(d.x + 0.5, d.y - 3.5, 9.0);
        match views.pawns.get(&key) {
            Some(&(e, c, sh)) => {
                if let Ok(mut t) = bodies.get_mut(e) {
                    t.translation = pos.translation;
                }
                if let Ok(mut t) = shadows.get_mut(sh) {
                    t.translation = shadow_pos.translation;
                }
                if let Ok((mut s, mut vis, mut t)) = carry.get_mut(c) {
                    t.translation = carry_pos.translation;
                    match p.carrying {
                        Some((item, _)) => {
                            let img = item_image(&mut views, &mut images, &g.content, item);
                            if s.image != img {
                                s.image = img;
                            }
                            vis.set_if_neq(Visibility::Inherited);
                        }
                        None => {
                            vis.set_if_neq(Visibility::Hidden);
                        }
                    }
                }
            }
            None => {
                let img = sprite_image(
                    &mut images,
                    &g.content.creature_rt[p.creature].sprite,
                    &p.slots,
                    true,
                );
                let e = commands
                    .spawn((Sprite::from_image(img), pos, PawnSprite))
                    .id();
                let c = commands
                    .spawn((
                        Sprite::default(),
                        carry_pos,
                        Visibility::Hidden,
                        CarrySprite,
                    ))
                    .id();
                let shadow = match &views.shadow_img {
                    Some(h) => h.clone(),
                    None => {
                        let h = shadow_image(&mut images);
                        views.shadow_img = Some(h.clone());
                        h
                    }
                };
                let sh = commands
                    .spawn((Sprite::from_image(shadow), shadow_pos, ShadowSprite))
                    .id();
                views.pawns.insert(key, (e, c, sh));
            }
        }
    }
}

/// Rotate a colour's hue by `turn` (fraction of a full turn).
fn hue_shift(c: [u8; 4], turn: f32) -> [u8; 4] {
    let (r, g, b) = (
        c[0] as f32 / 255.0,
        c[1] as f32 / 255.0,
        c[2] as f32 / 255.0,
    );
    let a = turn * std::f32::consts::TAU;
    let (s, co) = a.sin_cos();
    // Rotation around the grey axis.
    let k = (1.0 - co) / 3.0;
    let q = (1.0f32 / 3.0).sqrt() * s;
    let m = [
        [co + k, k - q, k + q],
        [k + q, co + k, k - q],
        [k - q, k + q, co + k],
    ];
    let f = |row: [f32; 3]| ((row[0] * r + row[1] * g + row[2] * b).clamp(0.0, 1.0) * 255.0) as u8;
    [f(m[0]), f(m[1]), f(m[2]), c[3]]
}

/// Wild animals: colour drifts with their colour gene, size with their size gene.
fn sync_animals(
    mut commands: Commands,
    game: Res<GameRes>,
    mut views: ResMut<Views>,
    mut images: ResMut<Assets<Image>>,
    mut q: Query<&mut Transform, With<AnimalSprite>>,
    clock: Res<SimClock>,
) {
    let g = &game.0;
    let live: HashSet<u32> = g.life.creatures.iter().map(|c| c.id).collect();
    let gone: Vec<u32> = views
        .animals
        .keys()
        .filter(|k| !live.contains(k))
        .copied()
        .collect();
    for id in gone {
        let (e, _) = views.animals.remove(&id).unwrap();
        commands.entity(e).despawn();
    }
    for c in &g.life.creatures {
        let def = g.content.creatures.by_idx(c.def);
        let n_slots = def
            .slots
            .values()
            .map(|v| v.len())
            .max()
            .unwrap_or(1)
            .max(1);
        let pick = (c.species as usize * 7 + c.id as usize) % n_slots;
        // 12 hue steps of colour drift.
        let hue = ((c.genes[pw_life::G_HUE] - 1.0) * 6.0)
            .round()
            .clamp(-6.0, 6.0) as i8;
        let key = (c.def, pick as u8, (hue + 6) as u8);
        let at = clock.draw_pos_life(c);
        let scale = (c.genes[pw_life::G_SIZE]
            * if c.age < 2 * pw_game::DAY_TICKS {
                0.75
            } else {
                1.0
            })
        .clamp(0.5, 2.0);
        let t = Transform::from_xyz(at.x, at.y, 9.5).with_scale(Vec3::splat(scale));
        match views.animals.get(&c.id) {
            Some(&(e, k)) if k == key => {
                if let Ok(mut tr) = q.get_mut(e) {
                    *tr = t;
                }
            }
            other => {
                if let Some(&(e, _)) = other {
                    commands.entity(e).despawn();
                }
                let img = match views.animal_images.get(&key) {
                    Some(h) => h.clone(),
                    None => {
                        let slots: std::collections::BTreeMap<String, String> = def
                            .slots
                            .iter()
                            .map(|(k, v)| {
                                let base = v
                                    .get(pick % v.len().max(1))
                                    .and_then(|h| parse_color(h))
                                    .unwrap_or([200, 0, 200, 255]);
                                let c = hue_shift(base, hue as f32 / 24.0);
                                (k.clone(), format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]))
                            })
                            .collect();
                        let h = sprite_image(
                            &mut images,
                            &g.content.creature_rt[c.def].sprite,
                            &slots,
                            true,
                        );
                        views.animal_images.insert(key, h.clone());
                        h
                    }
                };
                let e = commands
                    .spawn((Sprite::from_image(img), t, AnimalSprite))
                    .id();
                views.animals.insert(c.id, (e, key));
            }
        }
    }
}

fn draw_flashes(game: Res<GameRes>, mut gizmos: Gizmos) {
    let g = &game.0;
    for &(x, y, r, t) in &g.flashes {
        let age = g.tick.saturating_sub(t) as f32 / 20.0;
        if age >= 1.0 {
            continue;
        }
        let a = 1.0 - age;
        for k in 0..6 {
            let rr = r * (0.3 + 0.15 * k as f32) * (1.0 + age);
            gizmos.circle_2d(
                Isometry2d::from_translation(Vec2::new(x, y)),
                rr,
                Color::srgba(1.0, 0.97, 0.8, a * (1.0 - k as f32 / 6.0)),
            );
        }
    }
}

fn draw_zones(game: Res<GameRes>, settings: Res<RenderSettings>, mut gizmos: Gizmos) {
    if !settings.show_zones {
        return;
    }
    let g = &game.0;
    let size = Vec2::splat(TILE as f32 - 1.0);
    for t in &g.colony.stockpile {
        let (x, y) = t.center();
        gizmos.rect_2d(
            Isometry2d::from_translation(Vec2::new(x, y)),
            size,
            Color::srgba(1.0, 0.85, 0.3, 0.35),
        );
    }
    // Tribe lands: a faint ring in the tribe's colour.
    for tr in g.tribes.living() {
        let c = parse_color(&tr.color).unwrap_or([255, 255, 255, 255]);
        let (x, y) = tr.center.center();
        let r = (tr.radius as f32 + 0.5) * TILE as f32;
        gizmos.circle_2d(
            Isometry2d::from_translation(Vec2::new(x, y)),
            r,
            Color::srgba_u8(c[0], c[1], c[2], 70),
        );
        for t in &tr.colony.stockpile {
            let (x, y) = t.center();
            gizmos.rect_2d(
                Isometry2d::from_translation(Vec2::new(x, y)),
                size,
                Color::srgba_u8(c[0], c[1], c[2], 50),
            );
        }
    }
    for (t, d) in &g.colony.designations {
        let (x, y) = t.center();
        let c = parse_color(&g.content.jobs.by_idx(d.job).color).unwrap_or([255, 255, 255, 255]);
        let color = Color::srgba_u8(c[0], c[1], c[2], 220);
        gizmos.rect_2d(
            Isometry2d::from_translation(Vec2::new(x, y)),
            size - Vec2::splat(1.0),
            color,
        );
        if d.reserved.is_some() {
            gizmos.circle_2d(Isometry2d::from_translation(Vec2::new(x, y)), 1.2, color);
        }
    }
}
