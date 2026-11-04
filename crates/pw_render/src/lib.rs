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
    pub overlay_alpha: f32,
    pub show_zones: bool,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self { show_overlay: true, overlay_alpha: 1.0, show_zones: true }
    }
}

/// 0 = paused, 1..=3 = speed.
#[derive(Resource)]
pub struct SimClock {
    pub speed: u8,
    acc: f32,
    pub last_ticks: u32,
}

impl Default for SimClock {
    fn default() -> Self {
        Self { speed: 1, acc: 0.0, last_ticks: 0 }
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

struct ChunkView {
    base: Entity,
    over: Entity,
    base_img: Handle<Image>,
    over_img: Handle<Image>,
}

#[derive(Resource, Default)]
struct Views {
    chunks: HashMap<ChunkPos, ChunkView>,
    pawns: HashMap<u32, (Entity, Entity)>,
    items: HashMap<u32, (Entity, usize)>,
    blueprints: HashMap<u32, Entity>,
    item_images: HashMap<usize, Handle<Image>>,
    bp_images: HashMap<(usize, u64), (Handle<Image>, i32, i32, u32, u32)>,
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
                (run_sim, update_view, sync_chunks, sync_items, sync_blueprints, sync_pawns, draw_zones).chain(),
            );
    }
}

fn setup_camera(mut commands: Commands, game: Res<GameRes>) {
    let (x, y) = game.0.spawn_tile().center();
    commands.spawn((
        Camera2d,
        MainCamera,
        Projection::Orthographic(OrthographicProjection { scale: 0.25, ..OrthographicProjection::default_2d() }),
        Transform::from_xyz(x, y, 100.0),
    ));
}

/// Advance the simulation with a fixed tick rate scaled by the speed setting.
fn run_sim(time: Res<Time>, mut clock: ResMut<SimClock>, mut game: ResMut<GameRes>) {
    let m = clock.multiplier();
    clock.acc += time.delta_secs() * TPS * m;
    let mut n = 0;
    while clock.acc >= 1.0 && n < 24 {
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
    let (Ok((t, p)), Ok(w)) = (cam.single(), window.single()) else { return };
    let scale = match p {
        Projection::Orthographic(o) => o.scale,
        _ => 1.0,
    };
    let half = Vec2::new(w.width(), w.height()) * 0.5 * scale;
    let min = t.translation.truncate() - half;
    let max = t.translation.truncate() + half;
    game.0.view = Some((chunk_of(min.x as i32, min.y as i32), chunk_of(max.x as i32, max.y as i32)));
    game.0.stream_chunks();
}

fn new_image(w: u32, h: u32) -> Image {
    let mut img = Image::new_fill(
        Extent3d { width: w, height: h, depth_or_array_layers: 1 },
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

fn particle_color(content: &Content, m: MatId, seed: u64, x: i32, y: i32, layer: usize) -> [u8; 4] {
    let rt = content.mat_rt(m);
    let h = unit(hash3(seed ^ 0xC0105, x as i64, y as i64, layer as i64));
    shade(rt.color, 1.0 + (h - 0.5) * rt.variance)
}

/// Paint one chunk into its two images.
fn paint_chunk(chunk: &Chunk, content: &Content, seed: u64, base: &mut [u8], over: &mut [u8]) {
    let (ox, oy) = chunk.pos.origin();
    let solid = &chunk.mat[1];
    let overlay = &chunk.mat[2];
    let at = |v: &Vec<MatId>, lx: i32, ly: i32| -> MatId {
        if (0..CHUNK).contains(&lx) && (0..CHUNK).contains(&ly) { v[(ly * CHUNK + lx) as usize] } else { NONE_MAT }
    };
    for ly in 0..CHUNK {
        let row = (CHUNK - 1 - ly) as usize;
        for lx in 0..CHUNK {
            let i = (ly * CHUNK + lx) as usize;
            let p = (row * CHUNK as usize + lx as usize) * 4;
            let (x, y) = (ox + lx, oy + ly);
            let s = solid[i];
            let col = if s != NONE_MAT {
                // Fake height: lit top edge, dark bottom edge.
                let mut c = particle_color(content, s, seed, x, y, 1);
                let blocks = content.mat_rt(s).blocks;
                if blocks && at(solid, lx, ly - 1) == NONE_MAT {
                    c = shade(c, 0.62);
                } else if at(solid, lx, ly + 1) == NONE_MAT {
                    c = shade(c, 1.18);
                }
                c
            } else {
                let mut c = particle_color(content, chunk.mat[0][i], seed, x, y, 0);
                // Shadows from walls/rocks just below them and from canopies offset down-right.
                let above = at(solid, lx, ly + 1);
                if above != NONE_MAT && content.mat_rt(above).blocks {
                    c = shade(c, 0.6);
                } else if at(solid, lx - 1, ly + 2) != NONE_MAT && content.mat_rt(at(solid, lx - 1, ly + 2)).blocks {
                    c = shade(c, 0.8);
                } else if at(overlay, lx - 3, ly + 3) != NONE_MAT && content.mat_rt(at(overlay, lx - 3, ly + 3)).color[3] > 128 {
                    c = shade(c, 0.78);
                }
                c
            };
            base[p..p + 4].copy_from_slice(&[col[0], col[1], col[2], 255]);
            let o = overlay[i];
            if o != NONE_MAT {
                let mut c = particle_color(content, o, seed, x, y, 2);
                let edge = [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|&(dx, dy)| at(overlay, lx + dx, ly + dy) == NONE_MAT);
                if edge {
                    c = shade(c, 0.8);
                } else if at(overlay, lx + 1, ly + 1) == NONE_MAT {
                    c = shade(c, 1.12);
                }
                over[p..p + 4].copy_from_slice(&c);
            } else {
                over[p..p + 4].copy_from_slice(&[0, 0, 0, 0]);
            }
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
    // Remove views for unloaded chunks.
    let gone: Vec<ChunkPos> = views.chunks.keys().filter(|c| !g.world.chunks.contains_key(c)).copied().collect();
    for c in gone {
        let v = views.chunks.remove(&c).unwrap();
        commands.entity(v.base).despawn();
        commands.entity(v.over).despawn();
        images.remove(&v.base_img);
        images.remove(&v.over_img);
    }
    let mut painted = 0;
    for (cp, chunk) in g.world.chunks.iter_mut() {
        if !views.chunks.contains_key(cp) {
            let base_img = images.add(new_image(CHUNK as u32, CHUNK as u32));
            let over_img = images.add(new_image(CHUNK as u32, CHUNK as u32));
            let (x, y) = cp.origin();
            let center = Vec2::new(x as f32 + CHUNK as f32 / 2.0, y as f32 + CHUNK as f32 / 2.0);
            let base = commands
                .spawn((Sprite::from_image(base_img.clone()), Transform::from_xyz(center.x, center.y, 0.0), ChunkLayer))
                .id();
            let over = commands
                .spawn((Sprite::from_image(over_img.clone()), Transform::from_xyz(center.x, center.y, 20.0), ChunkLayer))
                .id();
            views.chunks.insert(*cp, ChunkView { base, over, base_img, over_img });
            chunk.render_dirty = true;
        }
        // Budget repaints per frame; the rest catch up next frame.
        if chunk.render_dirty && painted < 6 {
            let v = &views.chunks[cp];
            let (bh, oh) = (v.base_img.clone(), v.over_img.clone());
            let mut over_buf = vec![0u8; CHUNK_CELLS * 4];
            if let Some(mut img) = images.get_mut(&bh) {
                if let Some(data) = img.data.as_mut() {
                    paint_chunk(chunk, &content, seed, data, &mut over_buf);
                }
            }
            if let Some(mut img) = images.get_mut(&oh) {
                if let Some(data) = img.data.as_mut() {
                    data.copy_from_slice(&over_buf);
                }
            }
            chunk.render_dirty = false;
            painted += 1;
        }
    }
    // Overlay visibility / fade.
    for v in views.chunks.values() {
        if let Ok(mut vis) = vis.get_mut(v.over) {
            *vis = if settings.show_overlay { Visibility::Inherited } else { Visibility::Hidden };
        }
        if let Ok(mut s) = sprites.get_mut(v.over) {
            s.color = Color::srgba(1.0, 1.0, 1.0, settings.overlay_alpha);
        }
    }
}

fn sprite_image(images: &mut Assets<Image>, s: &SpriteRt, slots: &std::collections::BTreeMap<String, String>) -> Handle<Image> {
    let mut img = new_image(s.w.max(1), s.h.max(1));
    if let Some(data) = img.data.as_mut() {
        for (i, px) in s.pixels.iter().enumerate() {
            data[i * 4..i * 4 + 4].copy_from_slice(px);
        }
        for (i, slot) in &s.slots {
            let c = slots.get(slot).and_then(|h| parse_color(h)).unwrap_or([255, 0, 255, 255]);
            data[i * 4..i * 4 + 4].copy_from_slice(&c);
        }
    }
    images.add(img)
}

fn item_image(views: &mut Views, images: &mut Assets<Image>, content: &Content, item: usize) -> Handle<Image> {
    views
        .item_images
        .entry(item)
        .or_insert_with(|| sprite_image(images, &content.item_rt[item].sprite, &Default::default()))
        .clone()
}

fn check_content(views: &mut Views, content: &std::sync::Arc<Content>) {
    let ptr = std::sync::Arc::as_ptr(content) as usize;
    if views.content_ptr != ptr {
        views.content_ptr = ptr;
        views.item_images.clear();
        views.bp_images.clear();
    }
}

fn sync_items(mut commands: Commands, game: Res<GameRes>, mut views: ResMut<Views>, mut images: ResMut<Assets<Image>>) {
    let g = &game.0;
    check_content(&mut views, &g.content);
    let live: HashSet<u32> = g.colony.items.stacks.keys().copied().collect();
    let gone: Vec<u32> = views.items.keys().filter(|k| !live.contains(k)).copied().collect();
    for id in gone {
        let (e, _) = views.items.remove(&id).unwrap();
        commands.entity(e).despawn();
    }
    for s in g.colony.items.stacks.values() {
        let (x, y) = s.tile.center();
        let pos = Transform::from_xyz(x, y, 3.0);
        match views.items.get(&s.id) {
            Some(&(e, item)) if item == s.item => {
                commands.entity(e).insert(pos);
            }
            other => {
                if let Some(&(e, _)) = other {
                    commands.entity(e).despawn();
                }
                let img = item_image(&mut views, &mut images, &g.content, s.item);
                let e = commands.spawn((Sprite::from_image(img), pos, ItemSprite)).id();
                views.items.insert(s.id, (e, s.item));
            }
        }
    }
}

fn sync_blueprints(mut commands: Commands, game: Res<GameRes>, mut views: ResMut<Views>, mut images: ResMut<Assets<Image>>) {
    let g = &game.0;
    let gone: Vec<u32> = views.blueprints.keys().filter(|k| !g.colony.blueprints.contains_key(k)).copied().collect();
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
                let (minx, maxx) = cells.iter().fold((i32::MAX, i32::MIN), |a, c| (a.0.min(c.dx), a.1.max(c.dx)));
                let (miny, maxy) = cells.iter().fold((i32::MAX, i32::MIN), |a, c| (a.0.min(c.dy), a.1.max(c.dy)));
                let (w, h) = ((maxx - minx + 1).max(1) as u32, (maxy - miny + 1).max(1) as u32);
                let mut img = new_image(w, h);
                if let Some(data) = img.data.as_mut() {
                    for c in &cells {
                        let col = content.mat_rt(c.mat).color;
                        let (px, py) = ((c.dx - minx) as u32, (maxy - c.dy) as u32);
                        let i = ((py * w + px) * 4) as usize;
                        let blue = [(col[0] as f32 * 0.5 + 40.0) as u8, (col[1] as f32 * 0.6 + 60.0) as u8, 230, 150];
                        data[i..i + 4].copy_from_slice(&blue);
                    }
                }
                (images.add(img), minx, miny, w, h)
            })
            .clone();
        let (handle, minx, miny, w, h) = img;
        let cx = b.origin.0 as f32 + minx as f32 + w as f32 / 2.0;
        let cy = b.origin.1 as f32 + miny as f32 + h as f32 / 2.0;
        let e = commands.spawn((Sprite::from_image(handle), Transform::from_xyz(cx, cy, 4.0), BlueprintSprite)).id();
        views.blueprints.insert(b.id, e);
    }
}

fn sync_pawns(
    mut commands: Commands,
    game: Res<GameRes>,
    mut views: ResMut<Views>,
    mut images: ResMut<Assets<Image>>,
    mut carry: Query<(&mut Sprite, &mut Visibility), With<CarrySprite>>,
) {
    let g = &game.0;
    let live: HashSet<u32> = g.colony.pawns.iter().map(|p| p.id).collect();
    let gone: Vec<u32> = views.pawns.keys().filter(|k| !live.contains(k)).copied().collect();
    for id in gone {
        let (a, b) = views.pawns.remove(&id).unwrap();
        commands.entity(a).despawn();
        commands.entity(b).despawn();
    }
    for p in &g.colony.pawns {
        let pos = Transform::from_xyz(p.pos.0, p.pos.1, 10.0);
        let carry_pos = Transform::from_xyz(p.pos.0 + 3.0, p.pos.1 + 4.0, 11.0);
        match views.pawns.get(&p.id) {
            Some(&(e, c)) => {
                commands.entity(e).insert(pos);
                commands.entity(c).insert(carry_pos);
                if let Ok((mut s, mut vis)) = carry.get_mut(c) {
                    match p.carrying {
                        Some((item, _)) => {
                            s.image = item_image(&mut views, &mut images, &g.content, item);
                            *vis = Visibility::Inherited;
                        }
                        None => *vis = Visibility::Hidden,
                    }
                }
            }
            None => {
                let img = sprite_image(&mut images, &g.content.creature_rt[p.creature].sprite, &p.slots);
                let e = commands.spawn((Sprite::from_image(img), pos, PawnSprite)).id();
                let c = commands
                    .spawn((Sprite::default(), carry_pos, Visibility::Hidden, CarrySprite))
                    .id();
                views.pawns.insert(p.id, (e, c));
            }
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
        gizmos.rect_2d(Isometry2d::from_translation(Vec2::new(x, y)), size, Color::srgba(1.0, 0.85, 0.3, 0.35));
    }
    for (t, d) in &g.colony.designations {
        let (x, y) = t.center();
        let c = parse_color(&g.content.jobs.by_idx(d.job).color).unwrap_or([255, 255, 255, 255]);
        let color = Color::srgba_u8(c[0], c[1], c[2], 220);
        gizmos.rect_2d(Isometry2d::from_translation(Vec2::new(x, y)), size - Vec2::splat(1.0), color);
        if d.reserved.is_some() {
            gizmos.circle_2d(Isometry2d::from_translation(Vec2::new(x, y)), 1.2, color);
        }
    }
}
