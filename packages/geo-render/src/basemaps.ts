/** Géoplateforme basemaps (WMTS, Web Mercator) */
import type { Basemap, RasterOptions } from './types.ts';

const WMTS =
  'https://data.geopf.fr/wmts?SERVICE=WMTS&REQUEST=GetTile&VERSION=1.0.0&STYLE=normal&TILEMATRIXSET=PM' +
  '&TILEMATRIX={z}&TILEROW={y}&TILECOL={x}';
const PLAN = `${WMTS}&LAYER=GEOGRAPHICALGRIDSYSTEMS.PLANIGNV2&FORMAT=image/png`;

export const BASEMAPS: Record<Exclude<Basemap, 'none'>, RasterOptions> = {
  plan: { url: PLAN, maxZoom: 19 },
  'plan-grey': { url: PLAN, maxZoom: 19, saturation: 0 },
  ortho: { url: `${WMTS}&LAYER=ORTHOIMAGERY.ORTHOPHOTOS&FORMAT=image/jpeg`, maxZoom: 19 },
};

/** IGN hillshade, grey on transparent, zooms 6 to 17 */
export const HILLSHADE: RasterOptions = {
  url: `${WMTS}&LAYER=IGNF_ELEVATION.ELEVATIONGRIDCOVERAGE.SHADOW&FORMAT=image/png`,
  minZoom: 6,
  maxZoom: 17,
};

/** Sea color of each basemap, for the globe under the tiles (polar caps), RGB 0-1 */
export const SEA: Record<Basemap, [r: number, g: number, b: number]> = {
  plan: [0.667, 0.835, 0.914],
  'plan-grey': [0.792, 0.792, 0.792],
  ortho: [0.039, 0.231, 0.349],
  none: [0.85, 0.85, 0.83],
};
