//! A subtree file of implicit tiling (binary `subt` or JSON) handed to `Tree::add` under the placeholder of a quadtree
//! (or octree: first byte odd) tileset around Lille, then the placeholders it leaves expanded from memory and a
//! traversal. Input: the scheme byte, then the subtree file.
#![no_main]

use libfuzzer_sys::fuzz_target;
use tiles3d::tileset::{Readiness, Tree, select};

fuzz_target!(|data: &[u8]| {
    let [scheme, file @ ..] = data else {
        return;
    };
    let scheme = if scheme % 2 == 0 { "QUADTREE" } else { "OCTREE" };
    let r = f64::to_radians;
    let json = format!(
        r#"{{"asset":{{"version":"1.1"}},"geometricError":1000,"root":{{
            "boundingVolume":{{"region":[{},{},{},{},0,100]}},"geometricError":500,"refine":"REPLACE",
            "content":{{"uri":"content/{{level}}/{{x}}/{{y}}.glb"}},
            "implicitTiling":{{"subdivisionScheme":"{scheme}","subtreeLevels":3,"availableLevels":6,
                "subtrees":{{"uri":"subtrees/{{level}}/{{x}}/{{y}}.subtree"}}}}}}}}"#,
        r(3.0),
        r(50.58),
        r(3.12),
        r(50.68)
    );
    let mut tree = Tree::default();
    let root = tree
        .add(json.as_bytes(), "https://example.org/implicit/tileset.json", None)
        .expect("the fixed tileset");
    if tree
        .add(file, "https://example.org/implicit/subtrees/0/0/0.subtree", Some(root))
        .is_err()
    {
        return;
    }
    for _ in 0..64 {
        let Some(tile) = (0..tree.nodes.len()).find(|&t| tree.expandable(t)) else {
            break;
        };
        let _ = tree.add(&[], "", Some(tile));
    }
    // Deeper subtrees from the same bytes
    for _ in 0..4 {
        let Some(tile) = (0..tree.nodes.len())
            .rev()
            .find(|&t| matches!(tree.nodes[t].content, tiles3d::tileset::Content::Tileset(_)))
        else {
            break;
        };
        if tree
            .add(file, "https://example.org/implicit/subtrees/x.subtree", Some(tile))
            .is_err()
        {
            break;
        }
    }
    let mut camera = geo::camera::Camera::new(3.015, 50.5925, 17.0);
    camera.width = 640.0;
    camera.height = 400.0;
    let _ = select(&tree, root, &camera.view(), 16.0, &|i| {
        [Readiness::Ready, Readiness::Pending, Readiness::Empty][i % 3]
    });
});
