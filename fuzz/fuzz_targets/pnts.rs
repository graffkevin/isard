//! A `.pnts` tile as downloaded: `draco_requests` then `decode`, without the Draco geometry and with a stand-in for
//! it (of the count the tile says, or not). Any input must give points or an error, never a panic.
#![no_main]

use glam::DMat4;
use libfuzzer_sys::fuzz_target;
use tiles3d::decode::{DracoGeometry, decode, draco_requests};

fuzz_target!(|data: &[u8]| {
    let transform = DMat4::from_translation(glam::DVec3::new(1_215_012.0, -4_736_313.0, 4_081_605.0));
    let _ = decode(data, &transform, true, &[]);
    if let Ok(requests) = draco_requests(data)
        && !requests.is_empty()
    {
        // A stand-in geometry, its counts from the input's length: consistent with the tile or not
        let n = data.len() % 64;
        let geometry = DracoGeometry {
            positions: vec![1.0; n * 3],
            colors: vec![0.5; n * (3 + data.len() % 2)],
            normals: vec![0.0; n * 3],
            ..DracoGeometry::default()
        };
        let _ = decode(data, &DMat4::IDENTITY, true, &[geometry]);
    }
});
