//! The bytes of a COPC file as downloaded: its head (header and VLRs) read by `Copc::parse`, and as a hierarchy page
//! added under the root of the Paris tile's octree, its nodes placed, then the octree traversed. Any input must give
//! a head, nodes or an error, never a panic, a hang or a huge allocation.
#![no_main]

use geo::camera::Camera;
use glam::DVec3;
use libfuzzer_sys::fuzz_target;
use pointcloud::copc::{Copc, Head, Hierarchy, Info, Page, parse_page};
use pointcloud::traversal::{Readiness, Selection, select};

fuzz_target!(|data: &[u8]| {
    if let Ok(Head::Ready(copc)) = Copc::parse(data) {
        let _ = Hierarchy::new(copc.info);
    }
    let Ok(entries) = parse_page(data) else {
        return;
    };
    let info = Info {
        center: [652_500.0, 6_861_500.0, 522.21],
        halfsize: 500.0,
        spacing: 6.8,
        root_page: Page { offset: 0, size: 32 },
    };
    let mut tree = Hierarchy::new(info);
    let Ok(added) = tree.add_page(0, entries.clone()) else {
        return;
    };
    // Its pages again under the nodes that have one
    if let Some(&node) = added.iter().find(|&&i| tree.page(i).is_some()) {
        let _ = tree.add_page(node, entries);
    }
    for i in 0..tree.nodes.len() {
        let [x, y, z] = tree.center(i);
        let lon = 2.3434 + (x - 652_000.0) / 1000.0 * 0.0137;
        let lat = 48.8487 + (y - 6_861_000.0) / 1000.0 * 0.009;
        tree.place(i, DVec3::new(lon, lat, z + 44.0), 1.0, 1.0);
    }
    let mut camera = Camera::new(2.35, 48.853, 18.0);
    camera.width = 640.0;
    camera.height = 400.0;
    camera.pitch = 50.0;
    let mut out = Selection::default();
    select(&tree, &camera.view(), 1.5, 3_000_000, &|i| {
        [Readiness::Ready, Readiness::Pending, Readiness::Unavailable][i % 3]
    }, &mut out);
});
