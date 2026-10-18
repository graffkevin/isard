/** Géoplateforme basemaps (WMTS, Web Mercator), and the relief of each body */
import type { Basemap, RasterOptions } from './types.ts';

const WMTS =
  'https://data.geopf.fr/wmts?SERVICE=WMTS&REQUEST=GetTile&VERSION=1.0.0&STYLE=normal&TILEMATRIXSET=PM' +
  '&TILEMATRIX={z}&TILEROW={y}&TILECOL={x}';
const PLAN = `${WMTS}&LAYER=GEOGRAPHICALGRIDSYSTEMS.PLANIGNV2&FORMAT=image/png`;

export const BASEMAPS: Record<Exclude<Basemap, 'none' | 'plan-vector' | 'plan-grey-vector'>, RasterOptions> = {
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

/** IGN terrain model (RGE ALTI and its fill-ins), raw altitudes in metres from the Géoplateforme WMS */
export const TERRAIN_URL =
  'https://data.geopf.fr/wms-r/wms?SERVICE=WMS&VERSION=1.3.0&REQUEST=GetMap&LAYERS=ELEVATION.ELEVATIONGRIDCOVERAGE.HIGHRES' +
  '&STYLES=&CRS=EPSG:3857&BBOX={bbox}&WIDTH=256&HEIGHT=256&FORMAT=image/x-bil;bits=32';

/** The relief of the Moon and Mars by default: NASA's Trek image services (ArcGIS), raw 32-bit altitudes in metres
 * asked for in the body's Mercator. The Moon: LOLA's global model, 128 pixels per degree (237 m); Mars: MOLA's, 128 and
 * 64 pixels per degree merged (463 m), above the areoid */
export const BODY_TERRAINS = {
  moon: {
    url: 'https://trek.nasa.gov/moon/trekarcgis/rest/services/LRO_LOLA_DEM_Global_128ppd_v04/ImageServer',
    maxZoom: 8,
  },
  mars: {
    url: 'https://trek.nasa.gov/mars/trekarcgis/rest/services/mola128_mola64_merge_90Nto90S_SimpleC_clon0/ImageServer',
    maxZoom: 8,
  },
} as const;

/** An ArcGIS image service (a URL ending in `/ImageServer`) */
export const IMAGE_SERVER = /\/ImageServer\/?$/;

/**
 * The `exportImage` template of an ArcGIS image service for the relief: 256 × 256 raw 32-bit floats (`bsq`, followed
 * by its validity mask) of the box `{bbox}` in the Mercator of the body. On the Earth, Web Mercator (EPSG:3857);
 * elsewhere, a Mercator on the sphere of `radius` metres (Web Mercator's own formulas), given in WKT.
 */
export function imageServerTerrain(url: string, earth: boolean, radius: number): string {
  const crs = earth
    ? '3857'
    : encodeURIComponent(
        JSON.stringify({
          wkt:
            `PROJCS["Mercator",GEOGCS["GCS",DATUM["D",SPHEROID["S",${radius},0]],PRIMEM["Reference_Meridian",0],` +
            'UNIT["Degree",0.0174532925199433]],PROJECTION["Mercator"],PARAMETER["False_Easting",0],' +
            'PARAMETER["False_Northing",0],PARAMETER["Central_Meridian",0],PARAMETER["Standard_Parallel_1",0],' +
            'UNIT["Meter",1]]',
        }),
      );
  return (
    `${url.replace(/\/$/, '')}/exportImage?f=image&format=bsq&pixelType=F32&size=256,256` +
    `&bboxSR=${crs}&imageSR=${crs}&bbox={bbox}`
  );
}

/** Sea color of each basemap, for the globe under the tiles (polar caps), RGB 0-1 */
export const SEA: Record<Basemap, [r: number, g: number, b: number]> = {
  plan: [0.667, 0.835, 0.914],
  'plan-grey': [0.792, 0.792, 0.792],
  'plan-vector': [0.667, 0.835, 0.914],
  'plan-grey-vector': [0.792, 0.792, 0.792],
  ortho: [0.039, 0.231, 0.349],
  none: [0.85, 0.85, 0.83],
};
