//! Capsule-vs-triangle-soup collision, sized for corridor levels: a uniform
//! hash grid over the occluder triangles plus iterative push-out with slide
//! response. Collision geometry *is* the render occluder mesh — walls that
//! eat lines also stop the player.

use std::cell::RefCell;
use std::collections::HashMap;

use glam::Vec3;

thread_local! {
    /// Per-thread scratch for grid queries: the hot paths (slides, steer
    /// whiskers, line of sight, bolt sweeps) run hundreds of queries per
    /// frame, and this keeps them allocation-free in the steady state.
    /// Shared across soups — `begin` grows the mark table monotonically
    /// and the generation stamp invalidates stale marks for free.
    static SCRATCH: RefCell<QueryScratch> = RefCell::new(QueryScratch::default());
}

#[derive(Default)]
struct QueryScratch {
    /// Unique candidate ids gathered this query (sorted before use).
    ids: Vec<u32>,
    /// `mark[id] == generation` ⇒ id is already in `ids` this query.
    mark: Vec<u32>,
    generation: u32,
}

impl QueryScratch {
    fn begin(&mut self, triangle_count: usize) {
        self.ids.clear();
        if self.mark.len() < triangle_count {
            self.mark.resize(triangle_count, 0);
        }
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            // Wrapped after ~4 billion queries: stale marks could alias
            // the recycled generation, so reset them once.
            self.mark.fill(0);
            self.generation = 1;
        }
    }
}

const SLIDE_ITERATIONS: usize = 4;
/// A push-out steeper than this counts as standing on ground.
const GROUND_NORMAL_Y: f32 = 0.7;
/// Alternating-projection refinement steps for capsule↔triangle distance.
const CLOSEST_PAIR_ITERATIONS: usize = 4;

/// Static triangle collision world with a uniform grid accelerator.
pub struct TriangleSoup {
    triangles: Vec<[Vec3; 3]>,
    grid: HashMap<[i32; 3], Vec<u32>>,
    cell_size: f32,
}

impl TriangleSoup {
    pub fn new(vertices: &[Vec3], indices: &[u32], cell_size: f32) -> Self {
        let mut soup = Self {
            triangles: Vec::new(),
            grid: HashMap::new(),
            cell_size: cell_size.max(0.25),
        };
        for tri in indices.chunks_exact(3) {
            let triangle = [
                vertices[tri[0] as usize],
                vertices[tri[1] as usize],
                vertices[tri[2] as usize],
            ];
            let id = soup.triangles.len() as u32;
            let (lo, hi) = (
                triangle[0].min(triangle[1]).min(triangle[2]),
                triangle[0].max(triangle[1]).max(triangle[2]),
            );
            soup.triangles.push(triangle);
            for cell in soup.cells_covering(lo, hi) {
                soup.grid.entry(cell).or_default().push(id);
            }
        }
        soup
    }

    pub fn triangle_count(&self) -> usize {
        self.triangles.len()
    }

    fn cell_of(&self, p: Vec3) -> [i32; 3] {
        [
            (p.x / self.cell_size).floor() as i32,
            (p.y / self.cell_size).floor() as i32,
            (p.z / self.cell_size).floor() as i32,
        ]
    }

    fn cells_covering(&self, lo: Vec3, hi: Vec3) -> Vec<[i32; 3]> {
        let a = self.cell_of(lo);
        let b = self.cell_of(hi);
        let mut cells = Vec::new();
        for x in a[0]..=b[0] {
            for y in a[1]..=b[1] {
                for z in a[2]..=b[2] {
                    cells.push([x, y, z]);
                }
            }
        }
        cells
    }

    /// Run `visit` over the sorted, deduplicated triangle ids whose cells
    /// overlap the query box. Allocation-free in the steady state (the
    /// per-thread scratch grows to the busiest query and stays). Sorted so
    /// downstream iteration — and therefore `slide_capsule`'s sequential
    /// contact resolution — orders exactly like the old sort+dedup did.
    /// Queries must not nest (nothing here does).
    fn with_candidates<R>(&self, lo: Vec3, hi: Vec3, visit: impl FnOnce(&[u32]) -> R) -> R {
        SCRATCH.with(|cell| {
            let scratch = &mut *cell.borrow_mut();
            scratch.begin(self.triangles.len());
            let a = self.cell_of(lo);
            let b = self.cell_of(hi);
            for x in a[0]..=b[0] {
                for y in a[1]..=b[1] {
                    for z in a[2]..=b[2] {
                        let Some(bucket) = self.grid.get(&[x, y, z]) else {
                            continue;
                        };
                        for &id in bucket {
                            let mark = &mut scratch.mark[id as usize];
                            if *mark != scratch.generation {
                                *mark = scratch.generation;
                                scratch.ids.push(id);
                            }
                        }
                    }
                }
            }
            scratch.ids.sort_unstable();
            visit(&scratch.ids)
        })
    }

    /// Distance to the nearest triangle hit along `dir` (unit length),
    /// within `max_dist`. Used for hitscan weapons, projectile-vs-world
    /// checks, and line-of-sight tests.
    pub fn raycast(&self, origin: Vec3, dir: Vec3, max_dist: f32) -> Option<f32> {
        let end = origin + dir * max_dist;
        let pad = Vec3::splat(0.01);
        self.with_candidates(origin.min(end) - pad, origin.max(end) + pad, |ids| {
            let mut best: Option<f32> = None;
            for &id in ids {
                let [a, b, c] = self.triangles[id as usize];
                if let Some(t) = ray_triangle(origin, dir, a, b, c)
                    && t <= max_dist
                    && best.is_none_or(|prev| t < prev)
                {
                    best = Some(t);
                }
            }
            best
        })
    }

    /// True when the straight line from `from` to `to` is unobstructed.
    pub fn line_of_sight(&self, from: Vec3, to: Vec3) -> bool {
        let delta = to - from;
        let dist = delta.length();
        dist < 1e-4 || self.raycast(from, delta / dist, dist).is_none()
    }
}

/// Möller–Trumbore, double-sided, with a small epsilon so rays starting
/// exactly on a surface don't self-hit.
fn ray_triangle(origin: Vec3, dir: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let e1 = b - a;
    let e2 = c - a;
    let p = dir.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-8 {
        return None;
    }
    let inv = 1.0 / det;
    let s = origin - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = dir.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t > 1e-4).then_some(t)
}

/// Result of a capsule move through the soup.
#[derive(Debug, Clone, Copy)]
pub struct SlideResult {
    /// Corrected feet position after the move.
    pub position: Vec3,
    /// True if any resolved contact pushed mostly upward.
    pub grounded: bool,
}

/// Move a capsule (feet at `feet`, given radius/height) by `motion`,
/// pushing out of penetrations and sliding along surfaces.
pub fn slide_capsule(
    soup: &TriangleSoup,
    feet: Vec3,
    radius: f32,
    height: f32,
    motion: Vec3,
) -> SlideResult {
    let mut position = feet + motion;
    let mut grounded = false;
    let axis_top = (height - radius).max(radius);
    // The pre-move capsule midpoint is the trusted "outside" — it breaks
    // ties when a step lands the axis exactly on (or just past) a surface.
    let came_from = feet + Vec3::Y * (height * 0.5);

    for _ in 0..SLIDE_ITERATIONS {
        let p0 = position + Vec3::Y * radius;
        let p1 = position + Vec3::Y * axis_top;
        let pad = Vec3::splat(radius + 0.05);

        // Resolve the deepest penetration, then re-test — stable for the
        // shallow contacts a walking character produces.
        let best = soup.with_candidates(p0.min(p1) - pad, p0.max(p1) + pad, |ids| {
            let mut best: Option<Vec3> = None;
            for &id in ids {
                let [a, b, c] = soup.triangles[id as usize];
                if let Some(push) = capsule_triangle_pushout(p0, p1, radius, a, b, c, came_from)
                    && best.is_none_or(|b| push.length_squared() > b.length_squared())
                {
                    best = Some(push);
                }
            }
            best
        });
        let Some(push) = best else { break };
        position += push;
        if push.normalize_or_zero().y > GROUND_NORMAL_Y {
            grounded = true;
        }
    }

    SlideResult { position, grounded }
}

/// Push-out vector separating a capsule from a triangle, if penetrating.
/// `came_from` marks the side the capsule was on before moving: if the axis
/// has crossed the surface, the push goes back through it, not out the far
/// side.
fn capsule_triangle_pushout(
    p0: Vec3,
    p1: Vec3,
    radius: f32,
    a: Vec3,
    b: Vec3,
    c: Vec3,
    came_from: Vec3,
) -> Option<Vec3> {
    // Alternating projection converges to the closest segment↔triangle pair.
    let mut on_tri = (a + b + c) / 3.0;
    let mut on_seg = closest_point_on_segment(on_tri, p0, p1);
    for _ in 0..CLOSEST_PAIR_ITERATIONS {
        on_tri = closest_point_on_triangle(on_seg, a, b, c);
        on_seg = closest_point_on_segment(on_tri, p0, p1);
    }
    let delta = on_seg - on_tri;
    let distance = delta.length();
    if distance >= radius {
        return None;
    }
    if distance > 1e-6 {
        let mut direction = delta / distance;
        let mut depth = radius - distance;
        if direction.dot(came_from - on_tri) < 0.0 {
            // Axis ended up past the surface: push back to the origin side.
            direction = -direction;
            depth = radius + distance;
        }
        Some(direction * depth)
    } else {
        // Axis exactly on the surface: sign the normal by the origin side.
        let normal = (b - a).cross(c - a).normalize_or_zero();
        let direction = if normal.dot(came_from - on_tri) >= 0.0 {
            normal
        } else {
            -normal
        };
        Some(direction * radius)
    }
}

pub fn closest_point_on_segment(p: Vec3, a: Vec3, b: Vec3) -> Vec3 {
    let ab = b - a;
    let t = (p - a).dot(ab) / ab.length_squared().max(1e-12);
    a + ab * t.clamp(0.0, 1.0)
}

/// Ericson, *Real-Time Collision Detection* §5.1.5.
pub fn closest_point_on_triangle(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return a + ab * (d1 / (d1 - d3));
    }
    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return a + ac * (d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }
    let denom = 1.0 / (va + vb + vc);
    a + ab * (vb * denom) + ac * (vc * denom)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::vec3;

    #[test]
    fn closest_point_regions() {
        let (a, b, c) = (vec3(0.0, 0.0, 0.0), vec3(2.0, 0.0, 0.0), vec3(0.0, 2.0, 0.0));
        // Above the interior → projects onto the face.
        assert!(closest_point_on_triangle(vec3(0.5, 0.5, 3.0), a, b, c)
            .abs_diff_eq(vec3(0.5, 0.5, 0.0), 1e-6));
        // Beyond vertex a.
        assert_eq!(closest_point_on_triangle(vec3(-1.0, -1.0, 0.0), a, b, c), a);
        // Beside edge ab.
        assert!(closest_point_on_triangle(vec3(1.0, -5.0, 0.0), a, b, c)
            .abs_diff_eq(vec3(1.0, 0.0, 0.0), 1e-6));
    }

    fn floor_soup() -> TriangleSoup {
        let vertices = [
            vec3(-10.0, 0.0, -10.0),
            vec3(10.0, 0.0, -10.0),
            vec3(10.0, 0.0, 10.0),
            vec3(-10.0, 0.0, 10.0),
        ];
        TriangleSoup::new(&vertices, &[0, 1, 2, 0, 2, 3], 2.0)
    }

    #[test]
    fn falling_capsule_lands_on_floor_and_grounds() {
        let soup = floor_soup();
        // Feet sunk 0.2 into the floor after a gravity step.
        let result = slide_capsule(&soup, vec3(1.0, -0.2, 1.0), 0.35, 1.7, Vec3::ZERO);
        assert!(result.grounded);
        assert!(result.position.y.abs() < 1e-3, "feet on the floor plane");
    }

    #[test]
    fn wall_blocks_and_slides() {
        // Wall in the x=2 plane spanning y/z.
        let vertices = [
            vec3(2.0, -5.0, -10.0),
            vec3(2.0, 5.0, -10.0),
            vec3(2.0, 5.0, 10.0),
            vec3(2.0, -5.0, 10.0),
        ];
        let soup = TriangleSoup::new(&vertices, &[0, 1, 2, 0, 2, 3], 2.0);
        // Walk diagonally into the wall.
        let result = slide_capsule(&soup, vec3(1.5, 0.0, 0.0), 0.35, 1.7, vec3(0.5, 0.0, 0.5));
        assert!(
            result.position.x <= 2.0 - 0.35 + 1e-3,
            "kept out of the wall (x = {})",
            result.position.x
        );
        assert!(result.position.z > 0.4, "slid along the wall");
        assert!(!result.grounded);
    }

    #[test]
    fn open_space_is_a_no_op() {
        let soup = floor_soup();
        let result = slide_capsule(&soup, vec3(0.0, 3.0, 0.0), 0.35, 1.7, vec3(0.3, 0.0, 0.0));
        assert!(result.position.abs_diff_eq(vec3(0.3, 3.0, 0.0), 1e-6));
        assert!(!result.grounded);
    }

    #[test]
    fn candidate_gathering_matches_sort_dedup_semantics() {
        // A grid of quads so query boxes span many cells and triangles
        // land in several buckets each (dedup actually exercised).
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        for gx in 0..6 {
            for gz in 0..6 {
                let (x, z) = (gx as f32 * 3.0 - 9.0, gz as f32 * 3.0 - 9.0);
                let base = vertices.len() as u32;
                vertices.extend([
                    vec3(x, 0.0, z),
                    vec3(x + 3.0, 0.0, z),
                    vec3(x + 3.0, 0.0, z + 3.0),
                    vec3(x, 0.0, z + 3.0),
                ]);
                indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
            }
        }
        let soup = TriangleSoup::new(&vertices, &indices, 2.0);

        let brute = |lo: Vec3, hi: Vec3| -> Vec<u32> {
            // The old algorithm, verbatim: gather with duplicates, then
            // sort + dedup.
            let mut ids: Vec<u32> = soup
                .cells_covering(lo, hi)
                .into_iter()
                .filter_map(|cell| soup.grid.get(&cell))
                .flatten()
                .copied()
                .collect();
            ids.sort_unstable();
            ids.dedup();
            ids
        };

        let boxes = [
            (vec3(-9.5, -1.0, -9.5), vec3(9.5, 1.0, 9.5)), // everything
            (vec3(-1.0, -1.0, -1.0), vec3(1.0, 1.0, 1.0)), // center
            (vec3(-8.0, -1.0, 2.0), vec3(-2.0, 1.0, 8.0)), // off-center
            (vec3(50.0, 0.0, 50.0), vec3(51.0, 1.0, 51.0)), // empty
        ];
        // Repeat so the generation stamp advances between queries.
        for _ in 0..3 {
            for (lo, hi) in boxes {
                let got = soup.with_candidates(lo, hi, <[u32]>::to_vec);
                assert_eq!(got, brute(lo, hi), "box {lo:?}..{hi:?}");
            }
        }

        // A larger soup on the same thread grows the shared mark table.
        let big = TriangleSoup::new(
            &[vec3(0.0, 0.0, 0.0), vec3(1.0, 0.0, 0.0), vec3(0.0, 0.0, 1.0)],
            &[0, 1, 2],
            0.5,
        );
        let got = big.with_candidates(
            vec3(-1.0, -1.0, -1.0),
            vec3(2.0, 1.0, 2.0),
            <[u32]>::to_vec,
        );
        assert_eq!(got, vec![0]);
    }

    #[test]
    fn raycast_hits_the_floor_at_the_right_distance() {
        let soup = floor_soup();
        let t = soup
            .raycast(vec3(1.0, 5.0, 2.0), vec3(0.0, -1.0, 0.0), 100.0)
            .expect("straight down hits the floor");
        assert!((t - 5.0).abs() < 1e-4);
        // Parallel ray never lands; short ray stops before the floor.
        assert!(soup.raycast(vec3(0.0, 5.0, 0.0), vec3(1.0, 0.0, 0.0), 100.0).is_none());
        assert!(soup.raycast(vec3(0.0, 5.0, 0.0), vec3(0.0, -1.0, 0.0), 3.0).is_none());
    }

    #[test]
    fn line_of_sight_blocked_by_geometry() {
        let soup = floor_soup();
        assert!(!soup.line_of_sight(vec3(0.0, 2.0, 0.0), vec3(0.0, -2.0, 0.0)));
        assert!(soup.line_of_sight(vec3(0.0, 2.0, 0.0), vec3(5.0, 3.0, 5.0)));
    }
}
