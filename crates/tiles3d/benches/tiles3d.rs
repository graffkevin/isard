//! Benchmarks of the hot paths of `isard-tiles3d`: decoding the test tiles (b3dm, glb with meshopt and quantization),
//! the meshopt streams alone, the selection of a large tileset tree. `cargo bench -p isard-tiles3d`

use criterion::{Criterion, criterion_group, criterion_main};
use geo::camera::Camera;
use glam::{DMat4, DVec3};
use isard_tiles3d::decode;
use isard_tiles3d::meshopt::{self, Filter, Mode};
use isard_tiles3d::tileset::{self, Readiness, Tree};
use std::fmt::Write;
use std::hint::black_box;

const B3DM: &[u8] = include_bytes!("../tests/data/lille_L11_00.b3dm");
const NOQ: &[u8] = include_bytes!("../tests/data/lille_noq.glb");
const Q: &[u8] = include_bytes!("../tests/data/lille_q.glb");
const VPF: &[u8] = include_bytes!("../tests/data/lille_vpf.glb");

fn decoding(c: &mut Criterion) {
    // The glb were extracted from the b3dm without its RTC_CENTER: given back as the tile transform
    let rtc = DMat4::from_translation(DVec3::new(
        4_056_896.270_481_2,
        202_047.449_822_138,
        4_900_910.163_547_64,
    ));
    let mut group = c.benchmark_group("decode");

    group.bench_function("b3dm", |b| {
        b.iter(|| decode::decode(black_box(B3DM), &DMat4::IDENTITY, true, &[]));
    });
    for (name, bytes) in [
        ("glb meshopt", NOQ),
        ("glb meshopt quantized", Q),
        ("glb meshopt filtered", VPF),
    ] {
        group.bench_function(name, |b| b.iter(|| decode::decode(black_box(bytes), &rtc, true, &[])));
    }
    group.finish();
}

/// The meshopt-compressed buffer views of a glb: data, count, stride, mode, filter
fn meshopt_views(glb: &[u8]) -> Vec<(&[u8], usize, usize, Mode, Filter)> {
    let u32_at = |at: usize| u32::from_le_bytes([glb[at], glb[at + 1], glb[at + 2], glb[at + 3]]) as usize;
    let json_length = u32_at(12);
    let json: serde_json::Value = serde_json::from_slice(&glb[20..20 + json_length]).expect("glb JSON");
    let bin = &glb[20 + json_length + 8..];
    let mut out = Vec::new();

    for view in json["bufferViews"].as_array().into_iter().flatten() {
        let ext = &view["extensions"]["EXT_meshopt_compression"];

        if ext.is_null() {
            continue;
        }

        let at = |key: &str| ext[key].as_u64().unwrap_or(0) as usize;
        let (offset, length) = (at("byteOffset"), at("byteLength"));
        let mode = Mode::parse(ext["mode"].as_str().unwrap_or("")).expect("meshopt mode");
        let filter = Filter::parse(ext["filter"].as_str()).expect("meshopt filter");

        out.push((
            &bin[offset..offset + length],
            at("count"),
            at("byteStride"),
            mode,
            filter,
        ));
    }

    out
}

fn meshopt_streams(c: &mut Criterion) {
    let mut group = c.benchmark_group("meshopt");

    for (name, glb) in [("plain", NOQ), ("quantized", Q), ("filtered", VPF)] {
        let views = meshopt_views(glb);

        assert!(!views.is_empty(), "{name}: no meshopt view");
        group.bench_function(name, |b| {
            b.iter(|| {
                for &(data, count, stride, mode, filter) in &views {
                    let view = meshopt::View {
                        data,
                        count,
                        stride,
                        mode,
                        filter,
                    };

                    black_box(meshopt::decode(&view).expect("meshopt stream"));
                }
            });
        });
    }
    group.finish();
}

/// A quadtree tile over a region (radians) and its children down to `depth`
fn region_tile(region: [f64; 4], error: f64, depth: u32, out: &mut String) {
    let [w, s, e, n] = region;
    let _ = write!(
        out,
        r#"{{"boundingVolume":{{"region":[{w},{s},{e},{n},0,120]}},"geometricError":{error},"content":{{"uri":"t.glb"}}"#
    );

    if depth > 0 {
        out.push_str(r#","children":["#);

        let (mx, my) = (f64::midpoint(w, e), f64::midpoint(s, n));

        for (k, child) in [[w, s, mx, my], [mx, s, e, my], [w, my, mx, n], [mx, my, e, n]]
            .into_iter()
            .enumerate()
        {
            if k > 0 {
                out.push(',');
            }
            region_tile(child, error / 2.0, depth - 1, out);
        }
        out.push(']');
    }
    out.push('}');
}

fn tree_selection(c: &mut Criterion) {
    // 5,461 tiles (7 levels) over 40 km around Lille
    let (lon, lat) = (3.06f64.to_radians(), 50.63f64.to_radians());
    let half = 0.003;
    let mut root = String::new();

    region_tile([lon - half, lat - half, lon + half, lat + half], 256.0, 6, &mut root);

    let json = format!(r#"{{"asset":{{"version":"1.1"}},"geometricError":512,"root":{root}}}"#);
    let mut tree = Tree::default();
    let root = tree
        .add(json.as_bytes(), "https://example.org/tileset.json", None)
        .expect("tileset");
    let mut group = c.benchmark_group("tileset::select");

    for (name, zoom, pitch) in [("z13", 13.0, 0.0), ("z16 tilted", 16.0, 60.0)] {
        let mut camera = Camera::new(3.06, 50.63, zoom);

        camera.width = 1920.0;
        camera.height = 1080.0;
        camera.pitch = pitch;

        let view = camera.view();

        group.bench_function(name, |b| {
            b.iter(|| tileset::select(&tree, root, black_box(&view), 16.0, &|_| Readiness::Ready));
        });
    }
    group.finish();
}

criterion_group!(benches, decoding, meshopt_streams, tree_selection);
criterion_main!(benches);
