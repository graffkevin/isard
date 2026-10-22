//! Tests of the shapes: the fill meshes (caps, prism, cut along the grid, chunks) and the extruded blocks.

use super::triangulation::{CHUNKS_ACROSS, crossings, squares};
use super::*;
use glam::DVec3;

/// Points of the prism in 3D (z = level), and its triangles
fn faces(mesh: &Mesh, range: std::ops::Range<usize>) -> Vec<[DVec3; 3]> {
    mesh.indices[range]
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| {
            [t[0], t[1], t[2]].map(|i| {
                let v = mesh.vertices[i as usize];

                v.position().extend(f64::from(v.level) * 1e-3)
            })
        })
        .collect()
}

fn square_with_hole(size: f64) -> (Vec<DVec2>, Vec<u32>, Vec<u32>) {
    let o = DVec2::new(0.5, 0.33);
    let s = size;
    let points = vec![
        o,
        o + DVec2::new(s, 0.0),
        o + DVec2::new(s, s),
        o + DVec2::new(0.0, s),
        o,
        // The hole, the same way round (any way works)
        o + DVec2::new(s * 0.25, s * 0.25),
        o + DVec2::new(s * 0.75, s * 0.25),
        o + DVec2::new(s * 0.75, s * 0.75),
        o + DVec2::new(s * 0.25, s * 0.75),
    ];

    (points, vec![0, 5, 9], vec![0, 2])
}

fn mesh(size: f64) -> Mesh {
    let (points, ring_start, polygon_start) = square_with_hole(size);

    triangulate(&Polygons {
        points: &points,
        ring_start: &ring_start,
        polygon_start: &polygon_start,
    })
}

fn total_area(mesh: &Mesh) -> f64 {
    faces(mesh, 0..mesh.top)
        .iter()
        .map(|[a, b, c]| (b.truncate() - a.truncate()).perp_dot(c.truncate() - a.truncate()) / 2.0)
        .sum()
}

#[test]
fn the_top_cap_covers_the_polygon_without_its_hole() {
    let s = 1e-3;
    let m = mesh(s);

    assert!(
        (total_area(&m) - s * s * 0.75).abs() < s * s * 1e-9,
        "{}",
        total_area(&m)
    );
    // Every top triangle faces up
    assert!(
        faces(&m, 0..m.top)
            .iter()
            .all(|[a, b, c]| (*b - *a).cross(*c - *a).z > 0.0)
    );
}

#[test]
fn large_polygons_are_cut_along_the_grid() {
    // 10 squares across: no piece wider than a square, the area kept
    let s = SQUARE * 10.0;
    let m = mesh(s);

    assert!((total_area(&m) - s * s * 0.75).abs() < s * s * 1e-9);

    for [a, b, c] in faces(&m, 0..m.top) {
        let (lo, hi) = (a.min(b).min(c), a.max(b).max(c));

        assert!(hi.x - lo.x <= SQUARE * (1.0 + 1e-9) && hi.y - lo.y <= SQUARE * (1.0 + 1e-9));
    }
}

#[test]
fn the_prism_is_closed_and_turned_outwards() {
    let m = mesh(1e-3);
    let all = faces(&m, 0..m.indices.len());
    // Closed: the area vectors of its faces add up to nothing
    let sum: DVec3 = all.iter().map(|[a, b, c]| (*b - *a).cross(*c - *a)).sum();
    let scale: f64 = all.iter().map(|[a, b, c]| (*b - *a).cross(*c - *a).length()).sum();

    assert!(sum.length() < scale * 1e-9, "{sum:?}");

    // Outwards: each wall faces away from the middle of the ring it borders (the outer ring's walls away from the
    // center, the hole's towards it)
    let center = DVec2::new(0.5 + 5e-4, 0.33 + 5e-4);

    for [a, b, c] in faces(&m, m.walls..m.indices.len()) {
        let n = (b - a).cross(c - a).truncate();
        let middle = ((a + b + c) / 3.0).truncate();
        let outer = (middle - center).abs().max_element() > 3e-4;
        let away = n.dot(middle - center) > 0.0;

        assert_eq!(away, outer, "wall at {middle:?}");
    }
}

#[test]
fn extruded_blocks_have_a_roof_and_walls() {
    let (points, ring_start, polygon_start) = square_with_hole(1e-4);
    let polygons = Polygons {
        points: &points,
        ring_start: &ring_start,
        polygon_start: &polygon_start,
    };
    let b = extrude(&polygons, DVec2::new(0.5, 0.33));

    // 8 roof triangles (a square ring around a hole), 8 walls of 2 triangles
    assert_eq!(b.indices.len(), 8 * 3 + 8 * 6);

    let walls: Vec<_> = b.vertices.iter().filter(|v| v.level == 0.0).collect();

    assert_eq!(walls.len(), 16);

    // The walls facing north and west are lit, the others not (outside and in the hole: 4 walls, 2 feet each)
    let lit = b.vertices.iter().filter(|v| v.level == 0.0 && v.shade > 0.75).count();

    assert_eq!(lit, 8);
}

#[test]
fn small_layers_are_cut_finer_on_the_same_grid() {
    assert_eq!(square_for(DVec2::ZERO, DVec2::ONE), SQUARE);

    let s = square_for(DVec2::new(0.5, 0.3), DVec2::new(0.505, 0.302));

    assert!(
        s < SQUARE && 0.005 / s >= CHUNKS_ACROSS && 0.005 / s < 2.0 * CHUNKS_ACROSS,
        "{s}"
    );
    // A power of two of the squares: their grid lines are lines of the coarse grid
    assert_eq!((SQUARE / s).fract(), 0.0);
    assert!((SQUARE / s).log2().fract().abs() < 1e-12);
    assert_eq!(square_for(DVec2::ONE, DVec2::ONE), SQUARE);
}

#[test]
fn chunks_sort_triangles_by_square_keeping_their_order() {
    let size = 2e-3;
    let (points, ring_start, polygon_start) = square_with_hole(size);
    let polygons = Polygons {
        points: &points,
        ring_start: &ring_start,
        polygon_start: &polygon_start,
    };
    let square = square_for(points[0], points[2]);
    let mut m = triangulate_in(&polygons, square);
    let before = m.indices.clone();
    let place: std::collections::HashMap<[u32; 3], usize> = before
        .as_chunks::<3>()
        .0
        .iter()
        .enumerate()
        .map(|(k, t)| (*t, k))
        .collect();
    let area = total_area(&m);
    let c = chunk(&mut m, square);

    assert_eq!(c.top.len(), c.grid.len() + 1);
    assert_eq!(*c.top.last().unwrap() as usize, m.top);
    assert_eq!(*c.rest.last().unwrap() as usize, m.indices.len());
    assert!(c.grid.len() > 100, "{}", c.grid.len());

    // The same triangles, each in its square
    let mut sorted = before.as_chunks::<3>().0.to_vec();
    let mut after = m.indices.as_chunks::<3>().0.to_vec();

    sorted.sort_unstable();
    after.sort_unstable();
    assert_eq!(sorted, after);
    assert!((total_area(&m) - area).abs() < area * 1e-12);

    for s in 0..c.grid.len() {
        for t in m.indices[c.top[s] as usize..c.top[s + 1] as usize].as_chunks::<3>().0 {
            let p = t.iter().map(|&i| m.vertices[i as usize].position()).sum::<DVec2>() / 3.0;

            assert_eq!(c.grid.square(p) as usize, s);
        }

        // Order kept within a square
        let order: Vec<usize> = m.indices[c.top[s] as usize..c.top[s + 1] as usize]
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| place[t])
            .collect();

        assert!(order.is_sorted());
    }
}

#[test]
fn caps_and_walls_share_their_vertices() {
    let m = mesh(1e-3);

    // 8 points: the top cap indexes them, the bottom cap their copies at level 0, the walls stand on both
    assert_eq!((m.top_vertices, m.vertices.len()), (8, 16));
    assert_eq!((m.top, m.walls, m.indices.len()), (8 * 3, 16 * 3, 16 * 3 + 8 * 6));
    assert!(m.vertices[..8].iter().all(|v| v.level > 0.5));
    assert!(m.vertices[8..].iter().all(|v| v.level < 0.5));
    assert!(m.indices[..m.top].iter().all(|&i| i < 8));

    // Cut by the grid: the walls add their crossings, two vertices each, the pieces their points
    let big = mesh(SQUARE * 3.0);
    let n = big.vertices.len() as u32;

    assert!(big.indices.iter().all(|&i| i < n));
    assert!(big.indices[..big.top].iter().all(|&i| (i as usize) < big.top_vertices));
    assert!(
        big.vertices.len() * 3 < big.indices.len() * 2,
        "{} {}",
        big.vertices.len(),
        big.indices.len()
    );
}

#[test]
fn flat_meshes_are_the_top_cap_of_the_prism() {
    let (points, ring_start, polygon_start) = square_with_hole(SQUARE * 3.0);
    let polygons = Polygons {
        points: &points,
        ring_start: &ring_start,
        polygon_start: &polygon_start,
    };
    let prism = triangulate_with(&polygons, SQUARE, true);
    let flat = triangulate_with(&polygons, SQUARE, false);

    assert_eq!(
        (flat.top, flat.walls, flat.indices.len()),
        (prism.top, prism.top, prism.top)
    );
    assert_eq!(flat.vertices.len(), prism.top_vertices);
    assert_eq!(flat.vertices, prism.vertices[..prism.top_vertices]);
    assert_eq!(flat.indices, prism.indices[..prism.top]);
    assert_eq!((flat.min, flat.max), (prism.min, prism.max));
}

#[test]
fn polygons_starting_inside_the_previous_one_share_no_ring() {
    let (mut points, mut ring_start, mut polygon_start) = square_with_hole(SQUARE * 1.5);
    let o = DVec2::new(0.6, 0.4);

    points.extend([o, o + DVec2::X * 1e-3, o + DVec2::ONE * 1e-3, o]);
    ring_start.push(points.len() as u32);
    // Polygon 1 starts at ring 1 (inside polygon 0), polygon 2 at ring 3, past the rings
    polygon_start.extend([1, 3, 4]);

    let polygons = Polygons {
        points: &points,
        ring_start: &ring_start,
        polygon_start: &polygon_start,
    };
    let rings: Vec<_> = polygon_rings(&polygons).collect();

    assert_eq!(rings, [0..2, 2..2, 2..3, 3..3]);
    assert_eq!(
        polygons.rings(rings[2].clone()).map(<[DVec2]>::len).collect::<Vec<_>>(),
        [3]
    );

    let kept = OwnedPolygons::new(&polygons);
    let (a, b) = (triangulate(&polygons), triangulate(&kept.as_polygons()));

    assert_eq!((a.vertices, a.indices), (b.vertices, b.indices));
}

#[test]
fn overlapping_polygons_cost_only_the_rings_there_are() {
    // 10,000 polygons each claiming all 10,000 rings: each ring goes to one polygon only
    let points = [DVec2::ZERO, DVec2::X * 1e-6, DVec2::ONE * 1e-6];
    let ring_start: Vec<u32> = (0..=10_000).map(|_| 0).chain([3]).collect();
    let polygon_start: Vec<u32> = (0..=10_000).map(|p| if p % 2 == 0 { 0 } else { u32::MAX }).collect();
    let polygons = Polygons {
        points: &points,
        ring_start: &ring_start,
        polygon_start: &polygon_start,
    };
    let total: usize = polygon_rings(&polygons).map(|r| r.len()).sum();

    assert_eq!(total, ring_start.len() - 1);
    assert_eq!(extrude(&polygons, DVec2::ZERO).parts.len(), 1);
}

#[test]
fn blocks_are_sorted_by_square_polygon_by_polygon() {
    // A row of 40 small squares 50 m apart, from east to west
    let step = 50.0 / 40_075_016.686;
    let (mut points, mut ring_start, mut polygon_start) = (Vec::new(), vec![0u32], vec![0u32]);

    for k in (0..40).rev() {
        let p = DVec2::new(0.5 + f64::from(k) * step, 0.33);
        let s = step * 0.4;

        points.extend([p, p + DVec2::new(s, 0.0), p + DVec2::new(s, s), p + DVec2::new(0.0, s)]);
        ring_start.push(points.len() as u32);
        polygon_start.push(ring_start.len() as u32 - 1);
    }

    let polygons = Polygons {
        points: &points,
        ring_start: &ring_start,
        polygon_start: &polygon_start,
    };
    let mut blocks = extrude(&polygons, DVec2::new(0.5, 0.33));
    let before = blocks.indices.clone();
    let chunks = chunk_blocks(&mut blocks);

    assert_eq!(chunks.start.len(), chunks.grid.len() + 1);
    assert_eq!(*chunks.start.last().unwrap() as usize, before.len());
    assert!(chunks.reach > 0.0 && chunks.reach < step);

    let mut a = before.as_chunks::<3>().0.to_vec();
    let mut b = blocks.indices.as_chunks::<3>().0.to_vec();

    a.sort_unstable();
    b.sort_unstable();
    assert_eq!(a, b);

    // West first now (the last polygons), each polygon in its square, its triangles together, in their order
    let feature_of = |i: u32| blocks.vertices[i as usize].feature;

    assert!(feature_of(blocks.indices[0]) >= 38);

    let mut seen = Vec::new();

    for s in 0..chunks.grid.len() {
        let range = chunks.start[s] as usize..chunks.start[s + 1] as usize;
        let mut features: Vec<u32> = blocks.indices[range].iter().map(|&i| feature_of(i)).collect();

        features.dedup();
        assert!(features.is_sorted(), "{features:?}");
        seen.extend(features);
    }
    seen.sort_unstable();
    assert_eq!(seen, (0..40).collect::<Vec<_>>());
}

#[test]
fn malformed_polygons_do_not_panic_or_hang() {
    let huge = DVec2::new(1e300, 1e300);
    let points = [
        DVec2::ZERO,
        huge,
        DVec2::new(0.0, 1e300),
        DVec2::new(f64::NAN, 0.0),
        DVec2::new(f64::INFINITY, 1.0),
        DVec2::new(0.1, 0.1),
    ];

    // A huge triangle, a ring with NaN and infinity, rings whose starts point past the points
    for (ring_start, polygon_start) in [
        (vec![0, 3], vec![0, 1]),
        (vec![2, 6], vec![0, 1]),
        (vec![10, 4], vec![0, 1]),
        (vec![9, 20], vec![0, 1]),
        (vec![0, 3], vec![0, 5]),
        // Polygons claiming every ring there could be, or running backwards
        (vec![0, 3, u32::MAX], vec![0, u32::MAX, 0]),
        (vec![0, 3, 6], vec![0, 2, 0, 2, 1]),
        (vec![0, 3], vec![u32::MAX, 0, u32::MAX]),
    ] {
        let polygons = Polygons {
            points: &points,
            ring_start: &ring_start,
            polygon_start: &polygon_start,
        };
        let _ = triangulate(&polygons);
        let _ = extrude(&polygons, DVec2::ZERO);
    }
    assert_eq!(squares(DVec2::ZERO, huge, SQUARE).count(), 0);
    assert_eq!(squares(DVec2::NAN, DVec2::ONE, SQUARE).count(), 0);
    assert_eq!(crossings(DVec2::ZERO, huge, SQUARE), [0.0, 1.0]);
}

#[test]
fn positions_keep_the_f64_precision() {
    let p = DVec2::new(0.512_345_678_901_234, 0.354_321_098_765_432);
    let v = Vertex::new(p, 0, 1.0);

    assert!(v.position().distance(p) < 1e-14);
}
