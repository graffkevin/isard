//! The vertices of the points on the GPU, `STRIDE` bytes each, interleaved: x, y, z (f32, ECEF metres from the
//! node's anchor), red, green, blue and the class (u8), the intensity (u16), the return number and the number of
//! returns (u8). The 3D Tiles point clouds use the same vertices (class, intensity and returns 0).

use crate::frame::Grid;
use crate::las::{i32_at, u16_at};

/// Bytes per vertex
pub const STRIDE: usize = 20;
/// Offsets in a vertex: color and class, intensity, returns
pub const COLOR: usize = 12;
pub const INTENSITY: usize = 16;
pub const RETURNS: usize = 18;

/// What a vertex holds besides its position
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Attributes {
    pub rgb: [u8; 3],
    pub class: u8,
    pub intensity: u16,
    /// Return number, number of returns
    pub returns: [u8; 2],
}

/// Appends a vertex
pub fn push(out: &mut Vec<u8>, position: [f32; 3], a: &Attributes) {
    for v in position {
        out.extend(v.to_le_bytes());
    }
    out.extend(a.rgb);
    out.push(a.class);
    out.extend(a.intensity.to_le_bytes());
    out.extend(a.returns);
}

/// How the records of a file are laid out (its header)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    /// Point data record format, 6 to 8
    pub format: u8,
    pub record_length: usize,
    pub scale: [f64; 3],
    pub offset: [f64; 3],
}

impl Layout {
    /// The layout in the numbers the worker gets: format, record length, scale, offset
    #[must_use]
    pub fn numbers(&self) -> [f64; 8] {
        let [sx, sy, sz] = self.scale;
        let [ox, oy, oz] = self.offset;
        [
            f64::from(self.format),
            self.record_length as f64,
            sx,
            sy,
            sz,
            ox,
            oy,
            oz,
        ]
    }

    pub fn parse(numbers: &[f64]) -> Result<Self, String> {
        let bad = || "bad point layout".to_owned();
        let [format, length, sx, sy, sz, ox, oy, oz] = numbers.try_into().map_err(|_| bad())?;
        let layout = Self {
            format: format as u8,
            record_length: length as usize,
            scale: [sx, sy, sz],
            offset: [ox, oy, oz],
        };
        if !(6.0..=8.0).contains(&format) || layout.record_length < layout.minimum() || length > 1024.0 {
            return Err(bad());
        }
        if !numbers.iter().all(|v| v.is_finite()) {
            return Err(bad());
        }
        Ok(layout)
    }

    /// Bytes a record of its format holds at least (formats 6, 7, 8: 30, 36, 38)
    fn minimum(&self) -> usize {
        match self.format {
            6 => 30,
            7 => 36,
            _ => 38,
        }
    }

    fn has_rgb(&self) -> bool {
        self.format >= 7
    }
}

/// The vertices of the records of a chunk, positioned by the grid (its anchor their origin). Colors stored on 16 bits
/// are scaled to 8; a chunk whose colors all fit in 8 bits holds 8-bit colors (some writers store them so)
pub fn vertices(records: &[u8], layout: &Layout, grid: &Grid) -> Result<Vec<u8>, String> {
    let length = layout.record_length;
    if length < layout.minimum() || !records.len().is_multiple_of(length) {
        return Err("point records of a wrong size".into());
    }
    let count = records.len() / length;
    let rgb_at = |r: &[u8], i: usize| u16_at(r, 30 + 2 * i).unwrap_or(0);
    let shift = if layout.has_rgb() && records.chunks(length).all(|r| (0..3).all(|i| rgb_at(r, i) <= 255)) {
        0
    } else {
        8
    };
    let mut out = Vec::with_capacity(count * STRIDE);
    for r in records.chunks(length) {
        let [x, y, z] =
            [0, 1, 2].map(|i| f64::from(i32_at(r, i * 4).unwrap_or(0)) * layout.scale[i] + layout.offset[i]);
        let p = grid.point(x, y, z).as_vec3().to_array();
        let returns = r[14];
        let attributes = Attributes {
            rgb: if layout.has_rgb() {
                [0, 1, 2].map(|i| (rgb_at(r, i) >> shift) as u8)
            } else {
                [255; 3]
            },
            class: r[16],
            intensity: u16_at(r, 12).unwrap_or(0),
            returns: [returns & 0x0f, returns >> 4],
        };
        push(&mut out, p, &attributes);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec3;

    /// A grid where the CRS is ECEF itself around the origin: base = (x, y, 0), up = z
    fn identity() -> Grid {
        let nodes = Grid::nodes([0.0, 0.0], [100.0, 100.0], 2);
        Grid {
            min: [0.0, 0.0],
            max: [100.0, 100.0],
            side: 2,
            vertical_scale: 1.0,
            base: nodes.iter().map(|&[x, y]| DVec3::new(x, y, 0.0)).collect(),
            up: vec![DVec3::Z; 4],
        }
    }

    fn record(format: u8, x: i32, rgb: [u16; 3]) -> Vec<u8> {
        let mut r = vec![0u8; [30, 36, 38][usize::from(format - 6)]];
        r[0..4].copy_from_slice(&x.to_le_bytes());
        r[4..8].copy_from_slice(&200i32.to_le_bytes());
        r[8..12].copy_from_slice(&(-50i32).to_le_bytes());
        r[12..14].copy_from_slice(&999u16.to_le_bytes());
        r[14] = 0x32;
        r[16] = 6;
        if format >= 7 {
            for (i, c) in rgb.iter().enumerate() {
                r[30 + 2 * i..32 + 2 * i].copy_from_slice(&c.to_le_bytes());
            }
        }
        r
    }

    fn layout(format: u8) -> Layout {
        Layout {
            format,
            record_length: [30, 36, 38][usize::from(format - 6)],
            scale: [0.1; 3],
            offset: [10.0, 0.0, 5.0],
        }
    }

    #[test]
    fn records_become_vertices() {
        let records = [record(7, 100, [65535, 32768, 0]), record(7, 300, [256, 0, 0])].concat();
        let v = vertices(&records, &layout(7), &identity()).unwrap();
        assert_eq!(v.len(), 2 * STRIDE);
        let f = |at: usize| f32::from_le_bytes(v[at..at + 4].try_into().unwrap());
        // x = 100 × 0.1 + 10, y = 20, z = -5 + 5
        assert_eq!([f(0), f(4), f(8)], [20.0, 20.0, 0.0]);
        assert_eq!(&v[COLOR..COLOR + 4], &[255, 128, 0, 6]);
        assert_eq!(u16::from_le_bytes([v[INTENSITY], v[INTENSITY + 1]]), 999);
        assert_eq!(&v[RETURNS..RETURNS + 2], &[2, 3]);
        assert_eq!(f(STRIDE), 40.0);
    }

    #[test]
    fn colors_on_8_bits_are_kept() {
        let records = [record(8, 0, [200, 100, 50]), record(8, 1, [255, 0, 0])].concat();
        let v = vertices(&records, &layout(8), &identity()).unwrap();
        assert_eq!(&v[COLOR..COLOR + 3], &[200, 100, 50]);
        // Without colors: white
        let v = vertices(&record(6, 0, [0; 3]), &layout(6), &identity()).unwrap();
        assert_eq!(&v[COLOR..COLOR + 4], &[255, 255, 255, 6]);
    }

    #[test]
    fn layouts_are_checked() {
        let l = layout(7);
        assert_eq!(Layout::parse(&l.numbers()).unwrap(), l);
        assert!(Layout::parse(&[5.0, 30.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0]).is_err());
        assert!(Layout::parse(&[7.0, 30.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0]).is_err());
        assert!(Layout::parse(&[7.0, 36.0, f64::NAN, 1.0, 1.0, 0.0, 0.0, 0.0]).is_err());
        assert!(Layout::parse(&[7.0, 36.0]).is_err());
        assert!(vertices(&[0; 31], &layout(6), &identity()).is_err());
    }
}
