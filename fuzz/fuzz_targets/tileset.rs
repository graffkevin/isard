//! A tileset JSON as downloaded: `Tree::add` as the root, then grafted again under one of its tiles (an external
//! tileset), the placeholders of implicit tiling expanded from memory, and a traversal over Lille.
#![no_main]

use libfuzzer_sys::fuzz_target;
use tiles3d::tileset::{Readiness, Tree, select};

fuzz_target!(|data: &[u8]| {
    let mut tree = Tree::default();
    let Ok(root) = tree.add(data, "https://example.org/a/tileset.json?key=1", None) else {
        return;
    };
    let last = tree.nodes.len() - 1;
    let _ = tree.add(data, "https://example.org/a/b/external.json", Some(last));
    // Placeholders whose subtree is in memory (bounded: each expansion can add more)
    for _ in 0..64 {
        let Some(tile) = (0..tree.nodes.len()).find(|&t| tree.expandable(t)) else {
            break;
        };
        let _ = tree.add(&[], "", Some(tile));
    }
    let _ = select(&tree, root, &fuzz_view(), 16.0, &|i| {
        [Readiness::Ready, Readiness::Pending, Readiness::Empty][i % 3]
    });
});

fn fuzz_view() -> geo::camera::View {
    let mut camera = geo::camera::Camera::new(3.06, 50.63, 16.0);
    camera.width = 640.0;
    camera.height = 400.0;
    camera.view()
}
