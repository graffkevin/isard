//! Benchmark of the pure planning of a raster source: what to draw (stand-ins included) and what to load, frame after
//! frame, tiles arriving. `cargo bench -p isard`

use criterion::{Criterion, criterion_group, criterion_main};
use geo::camera::Camera;
use geo::tiles::{self, TileId};
use std::hint::black_box;

fn raster_planning(c: &mut Criterion) {
    let mut group = c.benchmark_group("raster plan");
    for (name, zoom, pitch) in [("flat z14", 14.0, 0.0), ("tilted z15 70°", 15.0, 70.0)] {
        let mut camera = Camera::new(2.35, 48.85, zoom);
        camera.width = 1920.0;
        camera.height = 1080.0;
        camera.pitch = pitch;
        let wanted = tiles::select(&camera.view(), 0, 19, 362.0);
        // A flight destination 40 km away
        camera.center.x += 0.001;
        let preload: Vec<TileId> = tiles::select(&camera.view(), 0, 19, 362.0)
            .into_iter()
            .take(48)
            .map(|s| s.tile)
            .collect();
        group.bench_function(format!("{name}, 60 frames"), |b| {
            b.iter(|| isard::bench::simulate_plans(black_box(&wanted), &preload, 60));
        });
    }
    group.finish();
}

criterion_group!(benches, raster_planning);
criterion_main!(benches);
