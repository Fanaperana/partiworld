//! A* over the tile grid (8 directions, no corner cutting).

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use pw_core::TilePos;
use pw_world::World;

#[derive(Copy, Clone, PartialEq)]
struct Node {
    f: f32,
    t: TilePos,
}
impl Eq for Node {}
impl Ord for Node {
    fn cmp(&self, o: &Self) -> Ordering {
        o.f.partial_cmp(&self.f).unwrap_or(Ordering::Equal).then_with(|| self.t.cmp(&o.t))
    }
}
impl PartialOrd for Node {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

fn octile(a: TilePos, b: TilePos) -> f32 {
    let dx = (a.x - b.x).abs() as f32;
    let dy = (a.y - b.y).abs() as f32;
    dx.max(dy) + (std::f32::consts::SQRT_2 - 1.0) * dx.min(dy)
}

/// Path from `start` (excluded) to the first tile where `goal` is true (included),
/// returned reversed so the next step is `path.last()`.
/// `hint` steers the search toward the goal area.
pub fn find_path(
    world: &World,
    start: TilePos,
    goal: impl Fn(TilePos) -> bool,
    hint: TilePos,
    max_nodes: usize,
) -> Option<Vec<TilePos>> {
    if goal(start) {
        return Some(Vec::new());
    }
    let mut open = BinaryHeap::new();
    let mut came: HashMap<TilePos, (f32, TilePos)> = HashMap::new();
    came.insert(start, (0.0, start));
    open.push(Node { f: octile(start, hint), t: start });
    let mut expanded = 0;
    while let Some(Node { t, .. }) = open.pop() {
        if goal(t) && t != start {
            let mut path = vec![t];
            let mut cur = t;
            while let Some(&(_, p)) = came.get(&cur) {
                if p == start {
                    break;
                }
                path.push(p);
                cur = p;
            }
            return Some(path);
        }
        expanded += 1;
        if expanded > max_nodes {
            return None;
        }
        let g = came[&t].0;
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)] {
            let n = t.offset(dx, dy);
            let Some(info) = world.tile(n) else { continue };
            if !info.walkable {
                continue;
            }
            if dx != 0 && dy != 0 && !(world.walkable(t.offset(dx, 0)) && world.walkable(t.offset(0, dy))) {
                continue;
            }
            let step = if dx != 0 && dy != 0 { std::f32::consts::SQRT_2 } else { 1.0 };
            let ng = g + step * info.cost.max(0.1);
            if came.get(&n).is_none_or(|&(og, _)| ng < og) {
                came.insert(n, (ng, t));
                open.push(Node { f: ng + octile(n, hint), t: n });
            }
        }
    }
    None
}

/// Nearest tile (breadth-first, up to `radius`) that passes `ok`.
pub fn nearest_tile(start: TilePos, radius: i32, ok: impl Fn(TilePos) -> bool) -> Option<TilePos> {
    if ok(start) {
        return Some(start);
    }
    for r in 1..=radius {
        let mut best: Option<(i32, TilePos)> = None;
        for y in -r..=r {
            for x in -r..=r {
                if x.abs() != r && y.abs() != r {
                    continue;
                }
                let t = start.offset(x, y);
                if ok(t) {
                    let d = x * x + y * y;
                    if best.is_none_or(|(bd, _)| d < bd) {
                        best = Some((d, t));
                    }
                }
            }
        }
        if let Some((_, t)) = best {
            return Some(t);
        }
    }
    None
}
