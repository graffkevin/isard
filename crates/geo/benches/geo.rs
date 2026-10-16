//! Benchmarks of the hot paths of `isard-geo`: tile selection, the pieces of a network, label placement, the
//! triangulation of fills. `cargo bench -p isard-geo` (add a filter: `cargo bench -p isard-geo -- select`)

use criterion::{Criterion, criterion_group, criterion_main};
use glam::DVec2;
use isard_geo::camera::Camera;
use isard_geo::collision::Grid;
use isard_geo::pieces::{self, Lines};
use isard_geo::shapes::{self, Polygons};
use std::hint::black_box;

fn camera(zoom: f64, pitch: f64, globe: bool) -> Camera {
    let mut c = Camera::new(2.35, 48.85, zoom);
    c.width = 1920.0;
    c.height = 1080.0;
    c.pitch = pitch;
    c.globe = globe;
    c
}

fn tile_selection(c: &mut Criterion) {
    let mut group = c.benchmark_group("tiles::select");
    for (name, camera) in [
        ("flat z14", camera(14.0, 0.0, false)),
        ("tilted z15 70°", camera(15.0, 70.0, false)),
        ("globe z3", camera(3.0, 30.0, true)),
    ] {
        let view = camera.view();
        group.bench_function(name, |b| {
            b.iter(|| isard_geo::tiles::select(black_box(&view), 0, 19, 362.0));
        });
    }
    group.finish();
}

/// A grid of streets, `n` × `rows` nodes 2e-6 apart, each edge bent through a middle point
fn network(n: u32, rows: u32) -> (Vec<DVec2>, Vec<u32>, Vec<u32>, Vec<DVec2>) {
    let nodes: Vec<DVec2> = (0..n * rows)
        .map(|k| DVec2::new(0.5 + f64::from(k % n) * 2e-6, 0.3 + f64::from(k / n) * 2e-6))
        .collect();
    let mut edges = Vec::new();
    let (mut start, mut points) = (vec![0u32], Vec::new());
    for k in 0..n * rows {
        for b in [k + 1, k + n] {
            if (b == k + 1 && k % n + 1 < n) || (b == k + n && b < n * rows) {
                edges.extend([k, b]);
                let (pa, pb) = (nodes[k as usize], nodes[b as usize]);
                points.extend([pa, (pa + pb) / 2.0 + DVec2::splat(3e-7), pb]);
                start.push(points.len() as u32);
            }
        }
    }
    (nodes, edges, start, points)
}

fn pieces_build(c: &mut Criterion) {
    // About 200,000 edges with their lines
    let (nodes, edges, start, points) = network(400, 250);
    let lines = Lines {
        start: &start,
        points: &points,
    };
    let mut group = c.benchmark_group("pieces");
    group.sample_size(20);
    group.bench_function("build 200k edges", |b| {
        b.iter(|| pieces::build(black_box(&nodes), &edges, Some(&lines), DVec2::new(0.5, 0.3)));
    });
    let built = pieces::build(&nodes, &edges, Some(&lines), DVec2::new(0.5, 0.3));
    let mut camera = Camera::new(0.0, 0.0, 17.0);
    camera.width = 1920.0;
    camera.height = 1080.0;
    camera.center = DVec2::new(0.5004, 0.3002);
    camera.pitch = 60.0;
    let view = camera.view();
    group.bench_function("visible", |b| {
        b.iter(|| built.visible(black_box(&view), (0.0, 0.0), 0.0));
    });
    group.finish();
}

/// A pseudo-random sequence in [0, 1), the same at every run
fn noise(count: usize) -> Vec<f64> {
    let mut state = 0x2545_f491_4f6c_dd1du64;
    (0..count)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 11) as f64 / (1u64 << 53) as f64
        })
        .collect()
}

fn collision_placement(c: &mut Criterion) {
    // 5,000 labels of 40 to 160 × 14 px over a 1920 × 1080 screen
    let r = noise(15_000);
    let boxes: Vec<[f64; 4]> = r
        .as_chunks::<3>()
        .0
        .iter()
        .map(|&[x, y, w]| {
            let (x, y) = (x * 1920.0, y * 1080.0);
            [x, y, x + 40.0 + w * 120.0, y + 14.0]
        })
        .collect();
    let mut grid = Grid::new(1920.0, 1080.0);
    c.bench_function("collision 5k labels", |b| {
        b.iter(|| {
            grid.reset(1920.0, 1080.0);
            boxes.iter().filter(|&&r| grid.place(black_box(r))).count()
        });
    });
}

fn fill_triangulation(c: &mut Criterion) {
    // 2,500 squares with a hole, 300 m apart, south-east of Paris
    let step = 300.0 / 40_075_016.686;
    let o = isard_geo::camera::mercator(2.35, 48.85);
    let (mut points, mut ring_start, mut polygon_start) = (Vec::new(), vec![0u32], vec![0u32]);
    for k in 0..2_500 {
        let p = o + DVec2::new(f64::from(k % 50), f64::from(k / 50)) * step;
        let s = step * 0.8;
        points.extend([p, p + DVec2::new(s, 0.0), p + DVec2::new(s, s), p + DVec2::new(0.0, s)]);
        ring_start.push(points.len() as u32);
        let q = p + DVec2::splat(s * 0.25);
        let h = s * 0.5;
        points.extend([q, q + DVec2::new(0.0, h), q + DVec2::new(h, h), q + DVec2::new(h, 0.0)]);
        ring_start.push(points.len() as u32);
        polygon_start.push(ring_start.len() as u32 - 1);
    }
    let polygons = Polygons {
        points: &points,
        ring_start: &ring_start,
        polygon_start: &polygon_start,
    };
    let (min, max) = (o, o + DVec2::splat(step * 50.0));
    let square = shapes::square_for(min, max);
    let mut group = c.benchmark_group("shapes 2.5k polygons");
    group.bench_function("triangulated", |b| b.iter(|| shapes::triangulate(black_box(&polygons))));
    group.bench_function("cut finer and chunked", |b| {
        b.iter(|| {
            let mut mesh = shapes::triangulate_in(black_box(&polygons), square);
            shapes::chunk(&mut mesh, square)
        });
    });
    group.finish();
}

criterion_group!(
    benches,
    tile_selection,
    pieces_build,
    collision_placement,
    fill_triangulation
);
criterion_main!(benches);
