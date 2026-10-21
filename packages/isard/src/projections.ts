/**
 * `isard/projections`: coordinate reference systems (Lambert-93, the CC zones, UTM, or any proj4 converter), GeoJSON
 * reprojected into longitudes and latitudes, and the tile grids of rasters served in another projection.
 *
 * ```ts
 * import { reproject, wmtsGrid } from 'isard/projections';
 * map.addFills({ data: reproject(parcels, 'EPSG:2154') });
 * map.addRaster({ url, grid: await wmtsGrid(capabilitiesUrl, 'PLANIGNV2.L93', '2154_10cm_6_20') });
 * ```
 */

export type { TileGrid, TileMatrix } from './basemap/grid.ts';
export { parseWmtsGrid, wmtsGrid } from './basemap/wmts.ts';
export {
  type Crs,
  type CrsLike,
  getCrs,
  lambertConformalConic,
  registerCrs,
  reproject,
  transverseMercator,
} from './data/crs.ts';
