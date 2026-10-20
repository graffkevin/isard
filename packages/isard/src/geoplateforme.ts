/**
 * The Géoplateforme's data (IGN, France; open, no key): its WMTS and WMS rasters as raster layers, its WFS features as
 * GeoJSON. Its basemaps, hillshade, terrain model and vector styles are named options of the map (`'ign-plan'`,
 * `hillshade`, `terrain: 'ign'`, `PLAN_STYLES`); the rasters credit the IGN by themselves.
 */
import type { FeatureCollection, Properties } from './features.ts';
import type { TileGrid, TileMatrix } from './grid.ts';
import type { RasterOptions } from './types.ts';

/** Credits of the Géoplateforme's data: its basemaps, hillshade, terrain model, rasters and features */
export const IGN_ATTRIBUTION =
  '<a href="https://www.ign.fr/">© IGN</a> – <a href="https://geoservices.ign.fr/">Géoplateforme</a>';

/** France (metropolitan, Corsica included) in longitudes and latitudes, where the Géoplateforme's Lambert-93 layers
 * lie */
const FRANCE: [number, number, number, number] = [-5.5, 41, 10, 51.5];

/**
 * The Géoplateforme's Lambert-93 tile matrix sets of 5 and 10 cm (`2154_10cm_6_20`, `2154_5cm_6_22`…: one pyramid,
 * 209 715.2 m per pixel at level 0, halved at each level, from the corner (0, 12 000 000)), levels `min` to `max`, over
 * France
 */
export function lambert93Grid(name: string, min: number, max: number): TileGrid {
  const matrices: TileMatrix[] = [];
  for (let level = min; level <= max; level += 1) {
    matrices.push({ id: level, resolution: 209715.2 / 2 ** level, origin: [0, 12000000] });
  }
  return { crs: 'EPSG:2154', matrices, bounds: FRANCE, name };
}

/**
 * A layer of the Géoplateforme's WMTS as raster options for `addRaster`, crediting the IGN:
 * `geoplateformeWmts('ORTHOIMAGERY.ORTHOPHOTOS', { format: 'image/jpeg' })`. The layer names are those of its
 * capabilities (https://data.geopf.fr/wmts?SERVICE=WMTS&REQUEST=GetCapabilities). In its Web Mercator grid (`PM`) by
 * default; a Lambert-93 layer in its matrix set (`matrixSet: '2154_10cm_6_20'`, a set of 5 or 10 cm, read from the
 * capabilities) or another `grid` (from `wmtsGrid`, its set named) is reprojected by the map
 */
export function geoplateformeWmts(
  layer: string,
  options: { format?: string; style?: string; matrixSet?: string } & Omit<RasterOptions, 'url'> = {},
): RasterOptions {
  const { format = 'image/png', style = 'normal', matrixSet, ...raster } = options;
  const set = (typeof raster.grid === 'object' ? raster.grid.name : undefined) ?? matrixSet ?? 'PM';
  const lambert = /^2154_(?:5|10)cm_(\d+)_(\d+)$/.exec(set);
  if (lambert && raster.grid === undefined) raster.grid = lambert93Grid(set, Number(lambert[1]), Number(lambert[2]));
  else if (set !== 'PM' && raster.grid === undefined) {
    throw new Error(`isard: the grid of the matrix set ${set}: wmtsGrid(capabilities, '${layer}', '${set}')`);
  }
  return {
    ...(raster.grid === undefined ? { maxZoom: 19 } : {}),
    attribution: IGN_ATTRIBUTION,
    ...raster,
    url:
      `https://data.geopf.fr/wmts?SERVICE=WMTS&REQUEST=GetTile&VERSION=1.0.0&STYLE=${encodeURIComponent(style)}` +
      `&TILEMATRIXSET=${encodeURIComponent(set)}&TILEMATRIX={z}&TILEROW={y}&TILECOL={x}` +
      `&LAYER=${encodeURIComponent(layer)}&FORMAT=${format}`,
  };
}

/**
 * Layers of the Géoplateforme's WMS (raster, `wms-r`) as raster options for `addRaster`, crediting the IGN: 256 px
 * images of each tile, transparent where empty. `geoplateformeWms('CADASTRALPARCELS.PARCELLAIRE_EXPRESS')`. Asked for
 * in EPSG:3857, or in `crs` (`'EPSG:2154'`: the box of each tile in Lambert-93, at the tile's `size`, reprojected by
 * the map; `'EPSG:4326'` is asked for as CRS:84, longitude first)
 */
export function geoplateformeWms(
  layers: string,
  options: { format?: string; styles?: string; crs?: string } & Omit<RasterOptions, 'url'> = {},
): RasterOptions {
  const { format = 'image/png', styles = '', crs = 'EPSG:3857', ...raster } = options;
  const projected = crs !== 'EPSG:3857';
  // WMS 1.3 puts the latitude first in EPSG:4326; {bbox} is x, y
  const asked = crs === 'EPSG:4326' ? 'CRS:84' : crs;
  return {
    maxZoom: 20,
    attribution: IGN_ATTRIBUTION,
    ...(projected ? { grid: { crs, ...(crs === 'EPSG:2154' ? { bounds: FRANCE } : {}) } } : {}),
    ...raster,
    url:
      `https://data.geopf.fr/wms-r/wms?SERVICE=WMS&VERSION=1.3.0&REQUEST=GetMap&CRS=${asked}&BBOX={bbox}` +
      // Reprojected, the image of each tile at its size
      `&${projected ? 'WIDTH={width}&HEIGHT={height}' : 'WIDTH=256&HEIGHT=256'}&TRANSPARENT=TRUE` +
      `&LAYERS=${encodeURIComponent(layers)}` +
      `&STYLES=${encodeURIComponent(styles)}&FORMAT=${format}`,
  };
}

/** Features the Géoplateforme's WFS returns per request at most */
const WFS_PAGE = 5000;

/**
 * Features of the Géoplateforme's WFS in a box (west, south, east, north, degrees), as GeoJSON in longitudes and
 * latitudes: `await geoplateformeWfs('BDTOPO_V3:batiment', [2.41, 48.84, 2.43, 48.85])`. At most `count` features
 * (default 50 000), asked by pages of 5000: the first one tells how many there are, the others are asked together.
 * The type names are those of its capabilities (BD TOPO, ADMIN EXPRESS…). With `srsName` (`'EPSG:2154'`), the
 * features come in that CRS (the box stays in degrees), the collection naming it in its `crs` member: the feature
 * layers reproject it by themselves
 */
export async function geoplateformeWfs<P = Properties>(
  typeName: string,
  [west, south, east, north]: [number, number, number, number],
  options: { count?: number; signal?: AbortSignal; srsName?: string } = {},
): Promise<FeatureCollection<P>> {
  const limit = Math.max(1, options.count ?? 50_000);
  const page = async (start: number, count: number) => {
    const url = new URL('https://data.geopf.fr/wfs/ows');
    url.search = new URLSearchParams({
      SERVICE: 'WFS',
      VERSION: '2.0.0',
      REQUEST: 'GetFeature',
      TYPENAMES: typeName,
      OUTPUTFORMAT: 'application/json',
      SRSNAME: options.srsName ?? 'EPSG:4326',
      // WFS 2.0 in EPSG:4326: latitude first
      BBOX: `${south},${west},${north},${east},urn:ogc:def:crs:EPSG::4326`,
      COUNT: String(count),
      // The first page as a single request always was
      ...(start > 0 ? { STARTINDEX: String(start) } : {}),
    }).toString();
    const response = await fetch(url, options.signal ? { signal: options.signal } : {});
    if (!response.ok) throw new Error(`isard: ${typeName}: ${response.status}`);
    return response.json() as Promise<FeatureCollection<P> & { numberMatched?: number | string }>;
  };
  const first = await page(0, Math.min(WFS_PAGE, limit));
  const matched = Number(first.numberMatched);
  const total = Math.min(Number.isFinite(matched) ? matched : 0, limit);
  if (first.features.length < WFS_PAGE || total <= first.features.length) return first;
  const starts: number[] = [];
  for (let start = first.features.length; start < total; start += WFS_PAGE) starts.push(start);
  const rest = await Promise.all(starts.map((start) => page(start, Math.min(WFS_PAGE, total - start))));
  // Pages may overlap where the server's order shifts: each feature once, by its id
  const seen = new Set<unknown>();
  const features = [first, ...rest]
    .flatMap((r) => r.features)
    .filter((f) => f.id === undefined || (!seen.has(f.id) && seen.add(f.id) !== undefined));
  return { ...first, features };
}
