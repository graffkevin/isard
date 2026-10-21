/** Where a raster layer's tiles come from: a PMTiles archive, Web Mercator tiles, or tiles reprojected from a grid. */

import { equirectangularGrid, gridLoader, gridZooms, type RemoteReprojection, resolveGrid } from '../basemap/grid.ts';
import { pmtilesRasterLoader } from '../basemap/vectortiles.ts';
import type { RasterOptions } from '../types.ts';

/** The loader of a raster layer's tiles (none: the engine fetches them) and its zooms, in Web Mercator levels */
export interface RasterSource {
  loader: RasterOptions['loader'];
  minZoom: number;
  maxZoom: number;
}

/** The loader of a raster layer's tiles and its zooms; `remote` reprojects in the workers */
export const rasterSource = (options: RasterOptions, remote: RemoteReprojection): RasterSource => {
  const { url, grid = 'mercator', size } = options;
  const { minZoom = 0, maxZoom = 19 } = options;
  // A PMTiles archive (`pmtiles://…`): its tiles read by range requests
  const loader = options.loader ?? pmtilesRasterLoader(url, size ?? 256);

  if (grid === 'mercator' || loader) return { loader, minZoom, maxZoom };
  if (grid === 'equirectangular') {
    // `minZoom` and `maxZoom` are levels of the source: its level n has the pixel size of Mercator level n + 1 at the
    // equator
    const reprojected = gridLoader(url, resolveGrid(equirectangularGrid(minZoom, maxZoom)), size ?? 256, remote);

    return { loader: reprojected, minZoom: minZoom + 1, maxZoom: maxZoom + 1 };
  }

  const resolved = resolveGrid(grid);
  const zooms = gridZooms(resolved);

  return {
    loader: gridLoader(url, resolved, size ?? 256, remote),
    minZoom: options.minZoom ?? zooms.minZoom,
    maxZoom: options.maxZoom ?? zooms.maxZoom,
  };
};
