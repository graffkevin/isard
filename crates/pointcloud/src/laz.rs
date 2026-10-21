//! The LAZ chunk of a COPC node decompressed into its point records: LAS 1.4 layered compression (point formats 6 to
//! 8, their colors, near infrared and extra bytes), by the `laz` crate. A chunk is decompressed alone: its point count
//! is known from the hierarchy, its layers from its first bytes.

use laz::record::{LayeredPointRecordDecompressor, RecordDecompressor};
use laz::{LazItemType, LazVlr};
use std::io::Cursor;

/// Points a chunk may hold: past it, refused before anything is allocated (COPC writers make chunks of a few hundred
/// thousand points at most)
pub const MAX_POINTS: usize = 1 << 22;

/// Decompresses the `count` point records of a chunk (`record_length` bytes each), compressed as the data of the LAZ
/// VLR `vlr` says
///
/// # Errors
///
/// Errors when `count` is past `MAX_POINTS`, on a malformed LAZ VLR, items other than the layered ones of
/// point formats 6 to 8, a record size other than `record_length`, or a chunk that does not decompress (one
/// shorter than its `count` points included)
pub fn decompress(chunk: &[u8], vlr: &[u8], record_length: usize, count: usize) -> Result<Vec<u8>, String> {
    if count > MAX_POINTS {
        return Err(format!("chunk of {count} points: more than {MAX_POINTS}"));
    }

    let vlr = LazVlr::read_from(vlr).map_err(|e| format!("LAZ VLR: {e}"))?;

    if vlr.items().first().map(laz::LazItem::item_type) != Some(LazItemType::Point14) {
        return Err("LAZ chunks of COPC are layered (point formats 6 to 8)".into());
    }

    let size = usize::try_from(vlr.items_size()).unwrap_or(usize::MAX);

    if size != record_length {
        return Err(format!("LAZ records of {size} bytes, the header says {record_length}"));
    }

    let mut decompressor = LayeredPointRecordDecompressor::new(Cursor::new(chunk));

    decompressor
        .set_fields_from(vlr.items())
        .map_err(|e| format!("LAZ items: {e}"))?;

    // A chunk ends where its points do: a chunk shorter than its count is an error (end of data)
    let mut out = vec![0; count * record_length];

    decompressor
        .decompress_many(&mut out)
        .map_err(|e| format!("LAZ chunk: {e}"))?;

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::copc::{Copc, Head};
    use crate::las::i32_at;
    use laz::LazItemRecordBuilder;
    use laz::record::{LayeredPointRecordCompressor, RecordCompressor};

    fn head(bytes: &[u8]) -> Copc {
        match Copc::parse(bytes).unwrap() {
            Head::Ready(copc) => *copc,
            Head::More(_) => panic!("the whole head is there"),
        }
    }

    /// The coordinates of a record (CRS units)
    fn xyz(record: &[u8], copc: &Copc) -> [f64; 3] {
        let h = &copc.header;

        [0, 1, 2].map(|i| f64::from(i32_at(record, i * 4).unwrap()) * h.scale[i] + h.offset[i])
    }

    /// Decodes a chunk of a fixture and checks every point lies in its node's cube
    fn decode_within(header: &[u8], chunk: &[u8], key: [u32; 4], count: usize) -> (Copc, Vec<u8>) {
        let copc = head(header);
        let length = usize::from(copc.header.record_length);
        let records = decompress(chunk, &copc.laz, length, count).unwrap();

        assert_eq!(records.len(), count * length);

        let [depth, x, y, z] = key;
        let (min, side) = crate::copc::Key { depth, x, y, z }.cube(&copc.info);

        for r in records.chunks(length) {
            let p = xyz(r, &copc);

            for i in 0..3 {
                assert!(
                    p[i] >= min[i] - 0.01 && p[i] <= min[i] + side + 0.01,
                    "{p:?} out of {min:?} + {side}"
                );
            }
        }

        (copc, records)
    }

    #[test]
    fn decodes_lidar_hd_chunks() {
        // Notre-Dame de Paris, IGN LiDAR HD (Licence Ouverte), point format 6 with its classes
        let header = include_bytes!("../tests/data/paris_header.bin");
        let (_, records) = decode_within(
            header,
            include_bytes!("../tests/data/paris_4-4-10-1.laz"),
            [4, 4, 10, 1],
            1543,
        );
        let classes: std::collections::BTreeSet<u8> = records.chunks(30).map(|r| r[16]).collect();

        // A roof of the cathedral: buildings (the classes of LiDAR HD: ground, vegetation, buildings, water, bridges,
        // 64 and up)
        assert_eq!(classes.into_iter().collect::<Vec<_>>(), [6]);
        assert!(records.chunks(30).any(|r| u16::from_le_bytes([r[12], r[13]]) > 0));
        decode_within(
            header,
            include_bytes!("../tests/data/paris_4-12-5-0.laz"),
            [4, 12, 5, 0],
            76,
        );
        decode_within(
            header,
            include_bytes!("../tests/data/paris_5-23-24-1.laz"),
            [5, 23, 24, 1],
            104,
        );
    }

    #[test]
    fn decodes_colors_of_format_7() {
        // Autzen (the COPC sample), point format 7: 16-bit colors
        let (_, records) = decode_within(
            include_bytes!("../tests/data/autzen_header.bin"),
            include_bytes!("../tests/data/autzen_5-16-2-1.laz"),
            [5, 16, 2, 1],
            121,
        );
        let red_max = records
            .chunks(36)
            .map(|r| u16::from_le_bytes([r[30], r[31]]))
            .max()
            .unwrap();

        assert!(red_max > 255, "16-bit colors: {red_max}");
    }

    /// A chunk compressed here: `count` records of point format 8 (with near infrared)
    fn compressed(count: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let items = LazItemRecordBuilder::default_for_point_format_id(8, 0).unwrap();
        let vlr = LazVlr::from_laz_items(items);
        let mut vlr_bytes = Vec::new();

        vlr.write_to(&mut vlr_bytes).unwrap();

        let length = vlr.items_size() as usize;
        let mut records = vec![0u8; count * length];

        for (i, r) in records.chunks_mut(length).enumerate() {
            r[0..4].copy_from_slice(&(i as i32 * 100).to_le_bytes());
            r[4..8].copy_from_slice(&(-(i as i32)).to_le_bytes());
            r[8..12].copy_from_slice(&7i32.to_le_bytes());
            r[12..14].copy_from_slice(&(i as u16 * 3).to_le_bytes());
            r[14] = 0x11;
            r[16] = (i % 20) as u8;
            r[30..32].copy_from_slice(&65535u16.to_le_bytes());
            r[36..38].copy_from_slice(&1234u16.to_le_bytes());
        }

        let mut compressor = LayeredPointRecordCompressor::new(Cursor::new(Vec::new()));

        compressor.set_fields_from(vlr.items()).unwrap();
        compressor.compress_many(&records).unwrap();
        compressor.done().unwrap();

        (compressor.into_inner().into_inner(), vlr_bytes, records)
    }

    #[test]
    fn decodes_format_8_as_compressed() {
        let (chunk, vlr, records) = compressed(500);

        assert_eq!(decompress(&chunk, &vlr, 38, 500).unwrap(), records);
        // The wrong record length, a count past the limit: errors before anything is decoded
        assert!(decompress(&chunk, &vlr, 30, 500).is_err());
        assert!(decompress(&chunk, &vlr, 38, MAX_POINTS + 1).is_err());
    }

    #[test]
    fn broken_chunks_are_errors() {
        let (chunk, vlr, _) = compressed(200);

        // Cut short, or more points asked for than it holds
        assert!(decompress(&chunk[..chunk.len() / 3], &vlr, 38, 200).is_err());
        assert!(decompress(&chunk[..3], &vlr, 38, 200).is_err());
        assert!(decompress(&[], &vlr, 38, 10).is_err());
        assert!(decompress(&chunk, &vlr[..10], 38, 200).is_err());
    }
}
