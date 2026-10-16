//! Page table of a tile pool over an extent: for each cell of a fixed level, the deepest loaded tile covering it.
//! Layers draped on the relief look their altitudes up through it (`render::drape`).

use crate::tiles::TileId;
use glam::DVec2;

/// Largest side of the table, in cells: wider extents take a coarser level
pub const MAX_SIDE: u32 = 512;

/// Cells of the table: (slot, level, 1 if any) of the deepest tile at or above `level` covering each cell, rows from
/// the north
#[derive(Debug, PartialEq)]
pub struct Table {
    pub level: u8,
    pub first: (u32, u32),
    pub size: (u32, u32),
    pub cells: Vec<[u8; 4]>,
}

/// Finest level of a table (cells counted in u32)
const MAX_LEVEL: u8 = 30;

/// Builds the table of the Mercator extent `min`..`max` (corners in any order) from the tiles of a pool, finest level
/// `max_z`. Slots past 255 do not fit a cell: such tiles are left out (a coarser tile stands in).
#[must_use]
pub fn table(min: DVec2, max: DVec2, max_z: u8, tiles: impl Iterator<Item = (TileId, u16)>) -> Table {
    let (min, max) = (min.min(max), min.max(max));
    // NaN goes to 0 (`as` saturates)
    let cell = |m: f64, level: u8| (m.clamp(0.0, 1.0 - 1e-12) * f64::from(1u32 << level)) as u32;
    let mut level = max_z.min(MAX_LEVEL);
    let widest = |level: u8| {
        cell(max.x, level)
            .saturating_sub(cell(min.x, level))
            .max(cell(max.y, level).saturating_sub(cell(min.y, level)))
    };
    while level > 0 && widest(level) >= MAX_SIDE {
        level -= 1;
    }
    let first = (cell(min.x, level), cell(min.y, level));
    let size = (
        cell(max.x, level).saturating_sub(first.0) + 1,
        cell(max.y, level).saturating_sub(first.1) + 1,
    );
    let mut cells = vec![[0u8; 4]; (size.0 * size.1) as usize];
    // Coarse tiles first, finer ones painted over them
    let mut tiles: Vec<(TileId, u16)> = tiles.filter(|(tile, _)| tile.z <= level).collect();
    tiles.sort_by_key(|(tile, _)| tile.z);
    for (tile, slot) in tiles {
        let Ok(slot) = u8::try_from(slot) else { continue };
        let span = 1u32 << (level - tile.z);
        let (x0, y0) = (tile.x.saturating_mul(span), tile.y.saturating_mul(span));
        let (x1, y1) = (x0.saturating_add(span), y0.saturating_add(span));
        let (cx0, cx1) = (x0.max(first.0), x1.min(first.0 + size.0));
        let (cy0, cy1) = (y0.max(first.1), y1.min(first.1 + size.1));
        for y in cy0..cy1 {
            let row = ((y - first.1) * size.0) as usize;
            for x in cx0..cx1 {
                cells[row + (x - first.0) as usize] = [slot, tile.z, 1, 0];
            }
        }
    }
    Table {
        level,
        first,
        size,
        cells,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deepest_tile_wins() {
        // Extent inside tile 10/512/340, table at level 12
        let (min, max) = (DVec2::new(0.5001, 0.3321), DVec2::new(0.5009, 0.3323));
        let tiles = [
            (TileId::new(6, 32, 21), 7u16),
            (TileId::new(12, (0.5001 * 4096.0) as u32, (0.3321 * 4096.0) as u32), 3),
        ];
        let t = table(min, max, 12, tiles.into_iter());
        assert_eq!(t.level, 12);
        assert_eq!(t.size, (4, 2));
        assert_eq!(t.cells[0], [3, 12, 1, 0]);
        assert_eq!(t.cells[1], [7, 6, 1, 0]);
    }

    #[test]
    fn wide_extents_take_coarser_levels() {
        let t = table(DVec2::new(0.4, 0.3), DVec2::new(0.6, 0.35), 15, std::iter::empty());
        assert!(t.size.0 <= MAX_SIDE && t.size.1 <= MAX_SIDE, "{:?}", t.size);
        assert!(t.level < 15);
        assert!(t.cells.iter().all(|c| c[2] == 0));
    }

    #[test]
    fn finer_tiles_than_the_table_are_left_out() {
        let t = table(
            DVec2::new(0.0, 0.0),
            DVec2::new(0.9, 0.9),
            15,
            [(TileId::new(15, 0, 0), 1u16)].into_iter(),
        );
        assert!(t.level < 15);
        assert!(t.cells.iter().all(|c| c[2] == 0));
    }

    #[test]
    fn odd_inputs_do_not_panic() {
        // Corners swapped, NaN, a slot past 255, a level past 31
        let t = table(
            DVec2::new(0.6, 0.4),
            DVec2::new(0.5, 0.3),
            40,
            [(TileId::new(0, 0, 0), 300u16)].into_iter(),
        );
        assert!(t.cells.iter().all(|c| c[2] == 0));
        let t = table(DVec2::NAN, DVec2::NAN, 12, std::iter::empty());
        assert_eq!(t.size, (1, 1));
    }
}
