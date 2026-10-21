//! A tileset JSON as downloaded: `Tree::add` as the root, then grafted again under one of its tiles (an external
//! tileset) and under the tiles naming a tileset (itself, maybe), the placeholders of implicit tiling expanded from
//! memory, and a traversal over Lille.
#![no_main]

use libfuzzer_sys::fuzz_target;
use tiles3d::tileset::{Content, Readiness, Tree, select};

fuzz_target!(|data: &[u8]| {
    let mut tree = Tree::default();
    let Ok(root) = tree.add(data, "https://example.org/a/tileset.json?key=1", None) else {
        return;
    };
    let last = tree.nodes.len() - 1;
    let _ = tree.add(data, "https://example.org/a/b/external.json", Some(last));
    // External tilesets followed by their own URLs, as the engine does: a tileset naming itself is refused
    for _ in 0..8 {
        let Some((tile, url)) = tree.nodes.iter().enumerate().rev().find_map(|(i, n)| match &n.content {
            Content::Tileset(url) if !tree.expandable(i) && n.children.is_empty() => Some((i, url.clone())),
            _ => None,
        }) else {
            break;
        };
        if tree.add(data, &url, Some(tile)).is_err() {
            break;
        }
    }
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
