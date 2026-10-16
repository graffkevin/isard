//! A `.b3dm` or `.glb` tile as downloaded: `draco_requests` then `decode` (without the Draco geometries, as when the
//! external decoder fails), y-up or z-up. Any input must give a mesh or an error, never a panic.
#![no_main]

use glam::DMat4;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = tiles3d::decode::draco_requests(data);
    // A transform as the tilesets give them (ECEF, far from the origin) and the identity
    let transform = DMat4::from_translation(glam::DVec3::new(4_045_000.0, 212_000.0, 4_910_000.0));
    let y_up = data.len() % 2 == 0;
    let _ = tiles3d::decode::decode(data, &transform, y_up, &[]);
    let _ = tiles3d::decode::decode(data, &DMat4::IDENTITY, !y_up, &[]);
});
