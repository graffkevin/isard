//! An `EXT_meshopt_compression` buffer view. Input: mode (1 byte), filter (1 byte), count (u16 LE), stride (1 byte),
//! then the compressed stream (seeds: the views of crates/tiles3d/tests/data/lille_*.glb in that layout).
#![no_main]

use libfuzzer_sys::fuzz_target;
use tiles3d::meshopt::{Filter, Mode, View, decode};

fuzz_target!(|data: &[u8]| {
    let [mode, filter, c0, c1, stride, stream @ ..] = data else {
        return;
    };
    let mode = [Mode::Attributes, Mode::Triangles, Mode::Indices][usize::from(*mode) % 3];
    let filter = [
        Filter::None,
        Filter::Octahedral,
        Filter::Quaternion,
        Filter::Exponential,
    ][usize::from(*filter) % 4];
    let view = View {
        data: stream,
        count: usize::from(u16::from_le_bytes([*c0, *c1])),
        stride: usize::from(*stride),
        mode,
        filter,
    };
    if let Ok(out) = decode(&view) {
        assert_eq!(out.len(), view.count * view.stride);
    }
});
