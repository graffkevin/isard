/**
 * The Géoplateforme's data (IGN, France; open, no key): its WMTS and WMS rasters as raster layers, its WFS features as
 * GeoJSON. Its basemaps, hillshade, terrain model and vector styles are named options of the map (`'ign-plan'`,
 * `hillshade`, `terrain: 'ign'`, `PLAN_STYLES`); the rasters credit the IGN by themselves.
 */
import type { FeatureCollection, Properties } from './features.ts';
import type { RasterOptions } from './types.ts';

/** Credits of the Géoplateforme's data: its basemaps, hillshade, terrain model, rasters and features */
export const IGN_ATTRIBUTION =
  '<a href="https://www.ign.fr/">© IGN</a> – <a href="https://geoservices.ign.fr/">Géoplateforme</a>';

/**
 * A layer of the Géoplateforme's WMTS (its Web Mercator grid, PM) as raster options for `addRaster`, crediting the
 * IGN: `geoplateformeWmts('ORTHOIMAGERY.ORTHOPHOTOS', { format: 'image/jpeg' })`. The layer names are those of its
 * capabilities (https://data.geopf.fr/wmts?SERVICE=WMTS&REQUEST=GetCapabilities)
 */
export function geoplateformeWmts(
  layer: string,
  options: { format?: string; style?: string } & Omit<RasterOptions, 'url'> = {},
): RasterOptions {
  const { format = 'image/png', style = 'normal', ...raster } = options;
  return {
    maxZoom: 19,
    attribution: IGN_ATTRIBUTION,
    ...raster,
    url:
      `https://data.geopf.fr/wmts?SERVICE=WMTS&REQUEST=GetTile&VERSION=1.0.0&STYLE=${encodeURIComponent(style)}` +
      `&TILEMATRIXSET=PM&TILEMATRIX={z}&TILEROW={y}&TILECOL={x}&LAYER=${encodeURIComponent(layer)}&FORMAT=${format}`,
  };
}

/**
 * Layers of the Géoplateforme's WMS (raster, `wms-r`) as raster options for `addRaster`, crediting the IGN: 256 px
 * images of each tile asked for in EPSG:3857, transparent where empty. `geoplateformeWms('CADASTRALPARCELS.PARCELLAIRE_EXPRESS')`
 */
export function geoplateformeWms(
  layers: string,
  options: { format?: string; styles?: string } & Omit<RasterOptions, 'url'> = {},
): RasterOptions {
  const { format = 'image/png', styles = '', ...raster } = options;
  return {
    maxZoom: 20,
    attribution: IGN_ATTRIBUTION,
    ...raster,
    url:
      'https://data.geopf.fr/wms-r/wms?SERVICE=WMS&VERSION=1.3.0&REQUEST=GetMap&CRS=EPSG:3857&BBOX={bbox}' +
      `&WIDTH=256&HEIGHT=256&TRANSPARENT=TRUE&LAYERS=${encodeURIComponent(layers)}` +
      `&STYLES=${encodeURIComponent(styles)}&FORMAT=${format}`,
  };
}

/**
 * Features of the Géoplateforme's WFS in a box (west, south, east, north, degrees), as GeoJSON in longitudes and
 * latitudes: `await geoplateformeWfs('BDTOPO_V3:batiment', [2.41, 48.84, 2.43, 48.85])`. At most `count` features
 * (default 5000). The type names are those of its capabilities (BD TOPO, ADMIN EXPRESS…)
 */
export async function geoplateformeWfs<P = Properties>(
  typeName: string,
  [west, south, east, north]: [number, number, number, number],
  options: { count?: number; signal?: AbortSignal } = {},
): Promise<FeatureCollection<P>> {
  const url = new URL('https://data.geopf.fr/wfs/ows');
  url.search = new URLSearchParams({
    SERVICE: 'WFS',
    VERSION: '2.0.0',
    REQUEST: 'GetFeature',
    TYPENAMES: typeName,
    OUTPUTFORMAT: 'application/json',
    SRSNAME: 'EPSG:4326',
    // WFS 2.0 in EPSG:4326: latitude first
    BBOX: `${south},${west},${north},${east},urn:ogc:def:crs:EPSG::4326`,
    COUNT: String(options.count ?? 5000),
  }).toString();
  const response = await fetch(url, options.signal ? { signal: options.signal } : {});
  if (!response.ok) throw new Error(`isard: ${typeName}: ${response.status}`);
  return response.json() as Promise<FeatureCollection<P>>;
}
