//! Decoder of `EXT_meshopt_compression` (glTF buffer views compressed with meshoptimizer, as gltfpack writes them):
//! the attribute, triangle and index bitstreams and the octahedral, quaternion and exponential filters, after the
//! specification (Appendix A and B of the extension). Every read is bounds-checked: a corrupt stream is an error, never
//! a panic.

#![forbid(unsafe_code)]

/// A compressed buffer view, as the extension describes it
pub struct View<'a> {
    /// The compressed bytes (the extension's `byteOffset` and `byteLength` in its buffer)
    pub data: &'a [u8],
    /// Elements (vertices, or indices) once decoded
    pub count: usize,
    /// Bytes per element once decoded
    pub stride: usize,
    /// Which bitstream the bytes are
    pub mode: Mode,
    /// The filter to undo after decoding (attributes only)
    pub filter: Filter,
}

/// The bitstream of a view (the extension's `mode`)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// Vertex attributes, byte deltas per block (Appendix A, mode 0)
    Attributes,
    /// Triangle list indices, edge and vertex FIFOs (mode 1)
    Triangles,
    /// Any other index sequence, varint deltas on two baselines (mode 2)
    Indices,
}

/// The filter of an attribute view (the extension's `filter`, Appendix B)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Filter {
    /// The decoded bytes are the values
    None,
    /// Unit vectors in octahedral encoding, to signed normalized x, y, z
    Octahedral,
    /// Unit quaternions with their largest component dropped, to signed normalized x, y, z, w
    Quaternion,
    /// 8-bit exponent and 24-bit mantissa, to f32
    Exponential,
}

impl Mode {
    /// The mode named by the extension (`ATTRIBUTES`, `TRIANGLES`, `INDICES`), an error for another name
    ///
    /// # Errors
    ///
    /// "meshopt mode …" for a name other than `ATTRIBUTES`, `TRIANGLES` and `INDICES`
    pub fn parse(mode: &str) -> Result<Self, String> {
        match mode {
            "ATTRIBUTES" => Ok(Self::Attributes),
            "TRIANGLES" => Ok(Self::Triangles),
            "INDICES" => Ok(Self::Indices),
            other => Err(format!("meshopt mode {other}")),
        }
    }
}

impl Filter {
    /// The filter named by the extension (`NONE` when absent), an error for an unknown name
    ///
    /// # Errors
    ///
    /// "meshopt filter …" for a name other than `NONE`, `OCTAHEDRAL`, `QUATERNION` and `EXPONENTIAL`
    pub fn parse(filter: Option<&str>) -> Result<Self, String> {
        match filter.unwrap_or("NONE") {
            "NONE" => Ok(Self::None),
            "OCTAHEDRAL" => Ok(Self::Octahedral),
            "QUATERNION" => Ok(Self::Quaternion),
            "EXPONENTIAL" => Ok(Self::Exponential),
            other => Err(format!("meshopt filter {other}")),
        }
    }
}

/// Decodes a view into `count * stride` bytes. The sizes are checked against the stream before anything is allocated:
/// attributes take a stride multiple of 4 up to 256, triangles and indices a stride of 2 or 4 (16- or 32-bit indices),
/// filters apply to attributes only.
///
/// # Errors
///
/// Errors on a filter on triangles or indices, a stride of triangles or indices other than 2 or 4, a stream of an
/// unsupported header, an attribute stride not a multiple of 4 up to 256, a filter's stride other than its own
/// (octahedral 4 or 8, quaternion 8), or a stream that does not hold `count * stride` bytes ("corrupt meshopt stream")
pub fn decode(view: &View) -> Result<Vec<u8>, String> {
    if view.filter != Filter::None && view.mode != Mode::Attributes {
        return Err("meshopt filters apply to attributes only".into());
    }
    if view.mode != Mode::Attributes && view.stride != 2 && view.stride != 4 {
        return Err("meshopt indices: stride 2 or 4".into());
    }

    let mut out = match view.mode {
        Mode::Attributes => vertex_buffer(view.data, view.count, view.stride)?,
        Mode::Triangles => index_buffer(view.data, view.count, view.stride)?,
        Mode::Indices => index_sequence(view.data, view.count, view.stride)?,
    };

    match view.filter {
        Filter::None => {}
        Filter::Exponential => exponential(&mut out),
        Filter::Octahedral => octahedral(&mut out, view.stride)?,
        Filter::Quaternion => quaternion(&mut out, view.stride)?,
    }

    Ok(out)
}

/// The error of every malformed stream
const CORRUPT: &str = "corrupt meshopt stream";

/// Bounds-checked reader
struct Reader<'a> {
    data: &'a [u8],
    at: usize,
    /// Reads stop here (the tail block of the stream is not part of the data)
    end: usize,
}

impl Reader<'_> {
    /// The next byte, an error at `end`
    fn byte(&mut self) -> Result<u8, String> {
        let b = *self.data.get(self.at).filter(|_| self.at < self.end).ok_or(CORRUPT)?;

        self.at += 1;

        Ok(b)
    }

    /// The next `n` bytes, an error past `end`
    fn bytes(&mut self, n: usize) -> Result<&[u8], String> {
        if n > self.end.saturating_sub(self.at) {
            return Err(CORRUPT.into());
        }

        let out = &self.data[self.at..self.at + n];

        self.at += n;

        Ok(out)
    }

    /// Unsigned LEB128 (varint-7)
    fn varint(&mut self) -> Result<u32, String> {
        let mut value = 0u32;

        for shift in (0..35).step_by(7) {
            let b = self.byte()?;

            value |= u32::from(b & 0x7f) << shift;

            if b & 0x80 == 0 {
                return Ok(value);
            }
        }

        Err(CORRUPT.into())
    }
}

// ── Mode 0: attributes ──

/// Mode 0: a header byte `0xa0`, then blocks of at most `max_block` vertices, each byte position of the stride
/// coded apart as zigzag deltas from the previous vertex, by groups of 16; the tail block holds the first baseline
fn vertex_buffer(data: &[u8], count: usize, stride: usize) -> Result<Vec<u8>, String> {
    if data.first() != Some(&0xa0) {
        return Err("meshopt attributes: unsupported header".into());
    }
    if stride == 0 || stride > 256 || !stride.is_multiple_of(4) {
        return Err("meshopt attributes: bad stride".into());
    }

    // Tail block: the baseline element, stored last, padded to 32 bytes
    let tail = stride.max(32);

    if data.len() < 1 + tail {
        return Err(CORRUPT.into());
    }

    let mut last = [0u8; 256];

    last[..stride].copy_from_slice(&data[data.len() - stride..]);

    let mut reader = Reader {
        data,
        at: 1,
        end: data.len() - tail,
    };
    let max_block = ((8192 / stride) & !15).min(256);
    // Smallest stream for `count` elements (each byte of a block has a header of 2 bits per group of 16): bounds the
    // output before allocating it
    let header = |block: usize| block.div_ceil(16).div_ceil(4);
    let smallest = (count / max_block)
        .checked_mul(header(max_block))
        .and_then(|n| n.checked_add(header(count % max_block)))
        .and_then(|n| n.checked_mul(stride));

    if smallest.is_none_or(|n| n > reader.end - reader.at) {
        return Err(CORRUPT.into());
    }

    let mut out = vec![0u8; count * stride];
    let mut deltas = [0u8; 256];
    let mut offset = 0;

    while offset < count {
        let block = max_block.min(count - offset);
        let groups = block.div_ceil(16);

        for byte in 0..stride {
            // Header: 2 bits per group, least significant first
            let mut header = [0u8; 4];

            header[..groups.div_ceil(4)].copy_from_slice(reader.bytes(groups.div_ceil(4))?);
            for group in 0..groups {
                let bits = (header[group / 4] >> ((group % 4) * 2)) & 3;

                group_deltas(&mut reader, bits, &mut deltas[group * 16..group * 16 + 16])?;
            }

            let mut value = last[byte];

            for (i, &delta) in deltas[..block].iter().enumerate() {
                // Zigzag decoding, wrapping 8-bit arithmetic
                value = value.wrapping_add((delta >> 1) ^ (delta & 1).wrapping_neg());
                out[(offset + i) * stride + byte] = value;
            }
            last[byte] = value;
        }
        offset += block;
    }
    if reader.at != reader.end {
        return Err(CORRUPT.into());
    }

    Ok(out)
}

/// One group of 16 byte deltas: all zero, 2-bit or 4-bit with sentinels, or raw bytes
fn group_deltas(reader: &mut Reader, bits: u8, out: &mut [u8]) -> Result<(), String> {
    match bits {
        0 => out.fill(0),
        3 => out.copy_from_slice(reader.bytes(16)?),
        _ => {
            let width = if bits == 1 { 2 } else { 4 };
            let sentinel = (1u8 << width) - 1;
            let mut packed = [0u8; 8];

            packed[..2 * width].copy_from_slice(reader.bytes(2 * width)?);

            for (i, slot) in out.iter_mut().enumerate() {
                // Most significant bits first inside each byte
                let bit = i * width;
                let value = (packed[bit / 8] >> (8 - width - bit % 8)) & sentinel;

                *slot = if value == sentinel { reader.byte()? } else { value };
            }
        }
    }

    Ok(())
}

// ── Mode 1: triangles ──

/// Mode 1: a header byte `0xe1`, one code byte per triangle, then the extra bytes (explicit indices, varints), and a
/// 16-byte `codeaux` table at the end. Each triangle reuses a recent edge (a FIFO of 16) and a recent vertex (a FIFO
/// of 16), or the next new index, or an explicit one
// One bitstream read in one place, case by case as the specification lists them
#[expect(
    clippy::too_many_lines,
    reason = "a port of meshoptimizer's index codec, kept whole to follow it"
)]
fn index_buffer(data: &[u8], count: usize, stride: usize) -> Result<Vec<u8>, String> {
    if data.first() != Some(&0xe1) {
        return Err("meshopt triangles: unsupported header".into());
    }
    if !count.is_multiple_of(3) || data.len() < 1 + count / 3 + 16 {
        return Err(CORRUPT.into());
    }

    let codeaux = &data[data.len() - 16..];
    let codes = &data[1..=count / 3];
    let mut extra = Reader {
        data,
        at: 1 + count / 3,
        end: data.len() - 16,
    };
    let mut edges = [(0u32, 0u32); 16];
    let mut vertices = [0u32; 16];
    let (mut edge_at, mut vertex_at) = (0usize, 0usize);
    let (mut next, mut last) = (0u32, 0u32);
    let mut out = Vec::with_capacity(count);
    let decode_index = |extra: &mut Reader, last: &mut u32| -> Result<u32, String> {
        let v = extra.varint()?;
        let delta = if v & 1 != 0 { !(v >> 1) } else { v >> 1 };

        *last = last.wrapping_add(delta);

        Ok(*last)
    };
    let edge = |edges: &[(u32, u32); 16], at: usize, i: usize| edges[(at.wrapping_sub(1 + i)) & 15];
    let vertex = |vertices: &[u32; 16], at: usize, i: usize| vertices[(at.wrapping_sub(1 + i)) & 15];

    for &code in codes {
        let (a, b, c);

        // Codes 0x00-0xef: edge `code >> 4` of the FIFO, the third vertex by `code & 15`: 0 the next new index, 1-12
        // a vertex of the FIFO, 13 / 14 the last explicit index -1 / +1, 15 an explicit index from the extra bytes
        if code < 0xf0 {
            // A recent edge and a third vertex
            (a, b) = edge(&edges, edge_at, usize::from(code >> 4));

            let low = code & 15;
            let pushed = match low {
                0 => {
                    c = next;
                    next = next.wrapping_add(1);

                    true
                }
                1..=0xc => {
                    c = vertex(&vertices, vertex_at, usize::from(low));

                    false
                }
                0xd | 0xe => {
                    c = if low == 0xd {
                        last.wrapping_sub(1)
                    } else {
                        last.wrapping_add(1)
                    };
                    last = c;

                    true
                }
                _ => {
                    c = decode_index(&mut extra, &mut last)?;

                    true
                }
            };

            if pushed {
                vertices[vertex_at & 15] = c;
                vertex_at += 1;
            }
            edges[edge_at & 15] = (c, b);
            edges[(edge_at + 1) & 15] = (a, c);
            edge_at += 2;
        } else {
            // Codes 0xf0-0xff: a triangle without edge reuse; its first vertex the next new index (0xff: explicit),
            // the other two by the nibbles of `codeaux[code & 15]` (0xfe, 0xff: an extra byte, 0 resetting `next`), each
            // 0 the next new index, 15 an explicit index (0xfe, 0xff), else a vertex of the FIFO
            let (zw, explicit) = if code < 0xfe {
                (codeaux[usize::from(code & 15)], false)
            } else {
                let zw = extra.byte()?;

                if zw == 0 {
                    next = 0;
                }

                (zw, true)
            };
            let (z, w) = (zw >> 4, zw & 15);

            a = if code == 0xff {
                decode_index(&mut extra, &mut last)?
            } else {
                next = next.wrapping_add(1);

                next.wrapping_sub(1)
            };

            let mut pick = |field: u8, extra: &mut Reader| -> Result<(u32, bool), String> {
                Ok(match field {
                    0 => {
                        next = next.wrapping_add(1);

                        (next.wrapping_sub(1), true)
                    }
                    0xf if explicit => (decode_index(extra, &mut last)?, true),
                    _ => (vertex(&vertices, vertex_at, usize::from(field) - 1), false),
                })
            };
            let (bb, push_b) = pick(z, &mut extra)?;
            let (cc, push_c) = pick(w, &mut extra)?;

            (b, c) = (bb, cc);
            vertices[vertex_at & 15] = a;
            vertex_at += 1;
            if push_b {
                vertices[vertex_at & 15] = b;
                vertex_at += 1;
            }
            if push_c {
                vertices[vertex_at & 15] = c;
                vertex_at += 1;
            }
            edges[edge_at & 15] = (b, a);
            edges[(edge_at + 1) & 15] = (c, b);
            edges[(edge_at + 2) & 15] = (a, c);
            edge_at += 3;
        }
        out.extend([a, b, c]);
    }
    if extra.at != extra.end {
        return Err(CORRUPT.into());
    }

    Ok(write_indices(&out, stride))
}

// ── Mode 2: indices ──

/// Mode 2: a header byte `0xd1`, one varint per index (bit 0 its baseline of two, bit 1 the sign of its zigzag delta),
/// and a 4-byte tail
fn index_sequence(data: &[u8], count: usize, stride: usize) -> Result<Vec<u8>, String> {
    if data.first() != Some(&0xd1) {
        return Err("meshopt indices: unsupported header".into());
    }
    // Each index takes at least a byte
    if data.len() < 1 + 4 || count > data.len() - 5 {
        return Err(CORRUPT.into());
    }

    let mut reader = Reader {
        data,
        at: 1,
        end: data.len() - 4,
    };
    let mut last = [0u32; 2];
    let mut out = Vec::with_capacity(count);

    for _ in 0..count {
        let v = reader.varint()?;
        let baseline = (v & 1) as usize;
        let delta = if v & 2 != 0 { !(v >> 2) } else { v >> 2 };

        last[baseline] = last[baseline].wrapping_add(delta);
        out.push(last[baseline]);
    }
    if reader.at != reader.end {
        return Err(CORRUPT.into());
    }

    Ok(write_indices(&out, stride))
}

/// Indices as 16-bit (stride 2) or 32-bit (stride 4) little-endian integers
fn write_indices(indices: &[u32], stride: usize) -> Vec<u8> {
    if stride == 2 {
        indices.iter().flat_map(|&i| (i as u16).to_le_bytes()).collect()
    } else {
        indices.iter().flat_map(|&i| i.to_le_bytes()).collect()
    }
}

// ── Filters ──

/// 2^e · m, from 32-bit integers with an 8-bit exponent and a 24-bit mantissa
fn exponential(data: &mut [u8]) {
    for chunk in data.as_chunks_mut::<4>().0 {
        let v = i32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        let (e, m) = (v >> 24, (v << 8) >> 8);
        let value = (m as f32) * f32::from_bits(((e + 127).clamp(0, 254) as u32) << 23);

        chunk.copy_from_slice(&value.to_le_bytes());
    }
}

/// Unit vectors in octahedral encoding, 8-bit (stride 4) or 16-bit (stride 8) components
fn octahedral(data: &mut [u8], stride: usize) -> Result<(), String> {
    let decode = |x: f32, y: f32, one: f32, max: f32| -> [f32; 3] {
        let (mut x, mut y) = (x / one, y / one);
        let z = 1.0 - x.abs() - y.abs();
        let t = z.min(0.0);

        x -= t.copysign(x);
        y -= t.copysign(y);

        let length = (x * x + y * y + z * z).sqrt().max(f32::MIN_POSITIVE);

        [
            (x / length * max).round(),
            (y / length * max).round(),
            (z / length * max).round(),
        ]
    };

    match stride {
        4 => {
            for v in data.as_chunks_mut::<4>().0 {
                let [x, y, z] = decode(
                    f32::from(v[0] as i8),
                    f32::from(v[1] as i8),
                    f32::from(v[2] as i8),
                    127.0,
                );

                (v[0], v[1], v[2]) = (x as i8 as u8, y as i8 as u8, z as i8 as u8);
            }
        }
        8 => {
            for v in data.as_chunks_mut::<8>().0 {
                let read = |i: usize| f32::from(i16::from_le_bytes([v[i * 2], v[i * 2 + 1]]));
                let [x, y, z] = decode(read(0), read(1), read(2), 32767.0);

                for (i, c) in [x, y, z].into_iter().enumerate() {
                    v[i * 2..i * 2 + 2].copy_from_slice(&(c as i16).to_le_bytes());
                }
            }
        }
        _ => return Err("meshopt octahedral filter: stride 4 or 8".into()),
    }

    Ok(())
}

/// Unit quaternions with the largest component dropped, 16-bit components
fn quaternion(data: &mut [u8], stride: usize) -> Result<(), String> {
    if stride != 8 {
        return Err("meshopt quaternion filter: stride 8".into());
    }

    let range = std::f32::consts::FRAC_1_SQRT_2;

    for v in data.as_chunks_mut::<8>().0 {
        let read = |i: usize| i16::from_le_bytes([v[i * 2], v[i * 2 + 1]]);
        let w_raw = read(3);
        let one = f32::from(w_raw | 3);
        let (x, y, z) = (
            f32::from(read(0)) / one * range,
            f32::from(read(1)) / one * range,
            f32::from(read(2)) / one * range,
        );
        let w = (1.0 - x * x - y * y - z * z).max(0.0).sqrt();
        let max = (w_raw & 3) as usize;
        let mut out = [0i16; 4];

        out[(max + 1) % 4] = (x * 32767.0).round() as i16;
        out[(max + 2) % 4] = (y * 32767.0).round() as i16;
        out[(max + 3) % 4] = (z * 32767.0).round() as i16;
        out[max] = (w * 32767.0).round() as i16;

        for (i, c) in out.into_iter().enumerate() {
            v[i * 2..i * 2 + 2].copy_from_slice(&c.to_le_bytes());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_example_of_4_bit_deltas() {
        // Appendix A: 16 deltas in 4-bit sentinel coding, two explicit bytes for elements 3 and 4
        let bytes = [0x17, 0x5f, 0xf0, 0xbc, 0x77, 0xa9, 0x21, 0x00, 0x34, 0xb5];
        let mut reader = Reader {
            data: &bytes,
            at: 0,
            end: bytes.len(),
        };
        let mut deltas = [0u8; 16];

        group_deltas(&mut reader, 2, &mut deltas).unwrap();

        let decoded: Vec<i8> = deltas
            .iter()
            .map(|&d| ((d >> 1) ^ (d & 1).wrapping_neg()) as i8)
            .collect();

        assert_eq!(decoded, [-1, -4, -3, 26, -91, 0, -6, 6, -4, -4, 5, -5, 1, -1, 0, 0]);
        assert_eq!(reader.at, bytes.len());
    }

    #[test]
    fn exponential_filter() {
        // e = -2, m = 6: 1.5
        let mut data = ((-2i32 << 24) | 6).to_le_bytes().to_vec();

        exponential(&mut data);
        assert_eq!(f32::from_le_bytes([data[0], data[1], data[2], data[3]]), 1.5);
    }

    #[test]
    fn index_sequence_with_two_baselines() {
        // 5, then 7 on baseline 0 (delta +5, +2), then 3 on baseline 1 (+3)
        let zig = |delta: i32, baseline: u32| (((delta << 1) ^ (delta >> 31)) as u32) << 1 | baseline;
        let mut data = vec![0xd1];

        for v in [zig(5, 0), zig(2, 0), zig(3, 1)] {
            data.push(v as u8);
        }
        data.extend([0; 4]);

        let out = index_sequence(&data, 3, 4).unwrap();
        let values: Vec<u32> = out.as_chunks::<4>().0.iter().map(|&c| u32::from_le_bytes(c)).collect();

        assert_eq!(values, [5, 7, 3]);
    }

    #[test]
    fn corrupt_streams_are_errors() {
        assert!(vertex_buffer(&[0xa0, 1, 2], 10, 4).is_err());
        assert!(index_buffer(&[0xe1], 3, 2).is_err());
        assert!(
            decode(&View {
                data: &[0x00; 64],
                count: 4,
                stride: 4,
                mode: Mode::Attributes,
                filter: Filter::None
            })
            .is_err()
        );
    }

    /// Encodes one group of 16 deltas in the given mode (0: zeros, 1: 2-bit, 2: 4-bit, 3: raw)
    fn encode_group(deltas: &[u8], bits: u8, out: &mut Vec<u8>) {
        match bits {
            0 => {}
            3 => out.extend_from_slice(deltas),
            _ => {
                let width = if bits == 1 { 2 } else { 4 };
                let sentinel = (1u8 << width) - 1;
                let mut packed = vec![0u8; 2 * width];
                let mut extra = Vec::new();

                for (i, &d) in deltas.iter().enumerate() {
                    let v = if d >= sentinel {
                        extra.push(d);

                        sentinel
                    } else {
                        d
                    };
                    let bit = i * width;

                    packed[bit / 8] |= v << (8 - width - bit % 8);
                }
                out.extend(packed);
                out.extend(extra);
            }
        }
    }

    /// A reference encoder of the attribute stream (the decoder read backwards), each group in its smallest mode
    fn encode_vertices(data: &[u8], count: usize, stride: usize) -> Vec<u8> {
        let mut out = vec![0xa0];
        let max_block = ((8192 / stride) & !15).min(256);
        let mut last = data[..stride].to_vec();
        let mut offset = 0;

        while offset < count {
            let block = max_block.min(count - offset);
            let groups = block.div_ceil(16);

            for byte in 0..stride {
                let mut deltas = vec![0u8; groups * 16];
                let mut previous = last[byte];

                for i in 0..block {
                    let value = data[(offset + i) * stride + byte];
                    let delta = value.wrapping_sub(previous) as i8;

                    // Zigzag
                    deltas[i] = ((delta << 1) ^ (delta >> 7)) as u8;
                    previous = value;
                }
                last[byte] = previous;

                let mut header = vec![0u8; groups.div_ceil(4)];
                let mut body = Vec::new();

                for (group, chunk) in deltas.chunks(16).enumerate() {
                    let bits = (0..4u8)
                        .min_by_key(|&bits| {
                            if bits == 0 && chunk.iter().any(|&d| d != 0) {
                                return usize::MAX;
                            }

                            let mut v = Vec::new();

                            encode_group(chunk, bits, &mut v);

                            v.len()
                        })
                        .unwrap();

                    header[group / 4] |= bits << ((group % 4) * 2);
                    encode_group(chunk, bits, &mut body);
                }
                out.extend(header);
                out.extend(body);
            }
            offset += block;
        }
        // Tail: the baseline, padded to 32 bytes
        out.extend(std::iter::repeat_n(0, stride.max(32) - stride));
        out.extend_from_slice(&data[..stride]);

        out
    }

    #[test]
    fn attributes_over_several_blocks_round_trip() {
        // 1000 vertices of 12 bytes (blocks of 256): smooth values (small deltas), noise (raw groups), constants
        let (count, stride) = (1000, 12);
        let mut r = 12_345u32;
        let data: Vec<u8> = (0..count * stride)
            .map(|i| {
                r = r.wrapping_mul(1_103_515_245).wrapping_add(12_345);

                match i % stride {
                    0..4 => (i / stride / 3) as u8,
                    4..8 => (r >> 16) as u8,
                    _ => 7,
                }
            })
            .collect();
        let encoded = encode_vertices(&data, count, stride);
        let view = |data: &[u8], count: usize| {
            decode(&View {
                data,
                count,
                stride,
                mode: Mode::Attributes,
                filter: Filter::None,
            })
        };

        assert_eq!(view(&encoded, count).unwrap(), data);
        // Far more vertices than encoded: errors, never a huge allocation
        assert!(view(&encoded, usize::MAX / 2).is_err());
        assert!(view(&encoded, 1 << 40).is_err());

        // Truncated or flipped streams: errors or garbage, never a panic
        for cut in (0..encoded.len()).step_by(7) {
            let _ = view(&encoded[..cut], count);
            let mut flipped = encoded.clone();

            flipped[cut] ^= 0x5a;

            let _ = view(&flipped, count);
        }
    }

    #[test]
    fn octahedral_filter() {
        // 8-bit: +z, +x and -z (folded corner); 16-bit: +y
        let mut data = vec![0, 0, 127, 0, 127, 0, 127, 0, 127, 127, 127, 0];

        octahedral(&mut data, 4).unwrap();

        let signed: Vec<i8> = data.iter().map(|&b| b as i8).collect();

        assert_eq!(signed, [0, 0, 127, 0, 127, 0, 0, 0, 0, 0, -127, 0]);

        let mut data: Vec<u8> = [0i16, 32767, 32767, 0].iter().flat_map(|v| v.to_le_bytes()).collect();

        octahedral(&mut data, 8).unwrap();

        let values: Vec<i16> = data.as_chunks::<2>().0.iter().map(|&c| i16::from_le_bytes(c)).collect();

        assert_eq!(values, [0, 32767, 0, 0]);
        assert!(octahedral(&mut [0; 12], 12).is_err());
    }

    #[test]
    fn quaternion_filter() {
        // Identity, then 90° about z, the largest component (w, index 3) dropped; scale 32767 in the high bits
        let one = 32767i16;
        let w = one | 3;
        let mut data: Vec<u8> = [0, 0, 0, w, 0, 0, one, w]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();

        quaternion(&mut data, 8).unwrap();

        let values: Vec<i16> = data.as_chunks::<2>().0.iter().map(|&c| i16::from_le_bytes(c)).collect();

        assert_eq!(values[..4], [0, 0, 0, 32767]);

        let h = (std::f32::consts::FRAC_1_SQRT_2 * 32767.0).round() as i16;

        assert_eq!(values[4..], [0, 0, h, h]);

        // The dropped component is the first one: the others shift
        let mut data: Vec<u8> = [0i16, 0, 0, (one & !3)].iter().flat_map(|v| v.to_le_bytes()).collect();

        quaternion(&mut data, 8).unwrap();

        let values: Vec<i16> = data.as_chunks::<2>().0.iter().map(|&c| i16::from_le_bytes(c)).collect();

        assert_eq!(values, [32767, 0, 0, 0]);
    }

    #[test]
    fn index_strides_and_filters_are_checked() {
        // Deltas +1, +2, +3 on baseline 0
        let data = [0xd1, 4, 8, 12, 0, 0, 0, 0];
        let view = |stride: usize, filter: Filter, count: usize| {
            decode(&View {
                data: &data,
                count,
                stride,
                mode: Mode::Indices,
                filter,
            })
        };

        // 16-bit output for stride 2
        assert_eq!(view(2, Filter::None, 3).unwrap(), [1, 0, 3, 0, 6, 0]);
        assert_eq!(view(4, Filter::None, 3).unwrap().len(), 12);
        assert!(view(3, Filter::None, 3).is_err());
        assert!(view(8, Filter::None, 3).is_err());
        assert!(view(4, Filter::Exponential, 3).is_err());
        // More indices than the stream can hold
        assert!(view(4, Filter::None, 1 << 40).is_err());
        assert!(index_buffer(&[0xe1; 20], 1 << 40, 4).is_err());
    }
}
