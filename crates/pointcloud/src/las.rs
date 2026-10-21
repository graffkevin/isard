//! The LAS 1.4 header of a file and its variable length records (VLR): what a COPC reader needs of them. Every field
//! is read at its offset after the length is checked.

/// Size of the LAS 1.4 header
pub const HEADER_SIZE: usize = 375;
/// Size of the header of a VLR
const VLR_HEADER: usize = 54;

/// What the reader keeps of a LAS header
#[derive(Clone, Debug, PartialEq)]
pub struct Header {
    pub version: (u8, u8),
    /// Point data record format (0 to 10), the compression bits of LAZ cleared
    pub point_format: u8,
    /// Bytes per point record
    pub record_length: u16,
    pub header_size: u16,
    /// Where the points start: the VLRs lie between the header and here
    pub point_data_offset: u32,
    pub vlr_count: u32,
    /// x, y, z of a record = its integer × scale + offset
    pub scale: [f64; 3],
    pub offset: [f64; 3],
    /// Bounds of the points (CRS units)
    pub min: [f64; 3],
    pub max: [f64; 3],
    pub point_count: u64,
}

pub(crate) fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

pub(crate) fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

pub(crate) fn i32_at(bytes: &[u8], at: usize) -> Option<i32> {
    Some(i32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

pub(crate) fn u64_at(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}

pub(crate) fn f64_at(bytes: &[u8], at: usize) -> Option<f64> {
    Some(f64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}

fn f64s<const N: usize>(bytes: &[u8], at: &[usize; N]) -> Option<[f64; N]> {
    let mut out = [0.0; N];

    for (o, &a) in out.iter_mut().zip(at) {
        *o = f64_at(bytes, a)?;
    }

    Some(out)
}

impl Header {
    /// The header at the start of a file (its first `HEADER_SIZE` bytes at least)
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.get(..4) != Some(b"LASF") {
            return Err("not a LAS file".into());
        }

        let truncated = || "truncated LAS header".to_owned();

        if bytes.len() < HEADER_SIZE {
            return Err(truncated());
        }

        let version = (bytes[24], bytes[25]);

        if version != (1, 4) {
            return Err(format!("LAS {}.{}: COPC is LAS 1.4", version.0, version.1));
        }

        let header_size = u16_at(bytes, 94).ok_or_else(truncated)?;
        let point_data_offset = u32_at(bytes, 96).ok_or_else(truncated)?;

        if usize::from(header_size) < HEADER_SIZE || point_data_offset < u32::from(header_size) {
            return Err("bad LAS header size".into());
        }

        let scale = f64s(bytes, &[131, 139, 147]).ok_or_else(truncated)?;
        let offset = f64s(bytes, &[155, 163, 171]).ok_or_else(truncated)?;
        let [max_x, min_x, max_y, min_y, max_z, min_z] =
            f64s(bytes, &[179, 187, 195, 203, 211, 219]).ok_or_else(truncated)?;

        if !scale.iter().chain(&offset).all(|v| v.is_finite()) || scale.contains(&0.0) {
            return Err("bad LAS scale or offset".into());
        }

        Ok(Self {
            version,
            point_format: bytes[104] & 0x3f,
            record_length: u16_at(bytes, 105).ok_or_else(truncated)?,
            header_size,
            point_data_offset,
            vlr_count: u32_at(bytes, 100).ok_or_else(truncated)?,
            scale,
            offset,
            min: [min_x, min_y, min_z],
            max: [max_x, max_y, max_z],
            point_count: u64_at(bytes, 247).ok_or_else(truncated)?,
        })
    }

    /// The VLRs, from the bytes of the file up to `point_data_offset` at least; a VLR past them is an error
    pub fn vlrs<'a>(&self, bytes: &'a [u8]) -> Result<Vec<Vlr<'a>>, String> {
        let end = (self.point_data_offset as usize).min(bytes.len());
        let mut at = usize::from(self.header_size);
        let mut out = Vec::new();

        for _ in 0..self.vlr_count {
            let header = bytes.get(at..at + VLR_HEADER).filter(|_| at + VLR_HEADER <= end);
            let header = header.ok_or("truncated VLR")?;
            let length = usize::from(u16_at(header, 20).ok_or("truncated VLR")?);
            let start = at + VLR_HEADER;
            let data = bytes
                .get(start..start + length)
                .filter(|_| start + length <= end)
                .ok_or("truncated VLR")?;

            out.push(Vlr {
                user: text(&header[2..18]),
                record: u16_at(header, 18).ok_or("truncated VLR")?,
                data,
            });
            at = start + length;
        }

        Ok(out)
    }
}

/// A variable length record: its user id, record id and data
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vlr<'a> {
    pub user: String,
    pub record: u16,
    pub data: &'a [u8],
}

/// A fixed-size text field: up to its first NUL, invalid UTF-8 replaced
pub(crate) fn text(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());

    String::from_utf8_lossy(&bytes[..end]).trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PARIS: &[u8] = include_bytes!("../tests/data/paris_header.bin");

    #[test]
    fn reads_the_header_of_lidar_hd() {
        let h = Header::parse(PARIS).unwrap();

        assert_eq!(h.version, (1, 4));
        assert_eq!((h.point_format, h.record_length), (6, 30));
        assert_eq!((h.header_size, h.point_data_offset, h.vlr_count), (375, 1418, 3));
        assert_eq!(h.scale, [0.01; 3]);
        assert_eq!(h.min[..2], [652_000.0, 6_861_000.0]);
        assert_eq!(h.max[..2], [653_000.0, 6_862_000.0]);
        assert_eq!(h.point_count, 15_511_179);

        let vlrs = h.vlrs(PARIS).unwrap();
        let ids: Vec<(&str, u16)> = vlrs.iter().map(|v| (v.user.as_str(), v.record)).collect();

        assert_eq!(ids, [("copc", 1), ("laszip encoded", 22204), ("LASF_Projection", 2112)]);
        assert_eq!(vlrs[0].data.len(), 160);
    }

    #[test]
    fn truncated_or_foreign_bytes_are_errors() {
        assert!(Header::parse(b"LASF").is_err());
        assert!(Header::parse(&PARIS[..300]).is_err());
        assert!(Header::parse(b"PK\x03\x04 and so on").is_err());

        let h = Header::parse(PARIS).unwrap();

        // The VLRs cut short
        assert!(h.vlrs(&PARIS[..1000]).is_err());

        // A VLR length running past the point data
        let mut bad = PARIS.to_vec();

        bad[375 + 20..375 + 22].copy_from_slice(&u16::MAX.to_le_bytes());
        assert!(h.vlrs(&bad).is_err());

        // Another version, a scale of zero
        let mut bad = PARIS.to_vec();

        bad[25] = 2;
        assert!(Header::parse(&bad).is_err());

        let mut bad = PARIS.to_vec();

        bad[131..139].copy_from_slice(&0.0f64.to_le_bytes());
        assert!(Header::parse(&bad).is_err());
    }
}
