/** The basemaps by name: OpenFreeMap's vector styles (OpenStreetMap data, worldwide) and the Géoplateforme's (IGN,
 * France), the credits of the sources, and the relief of each body */
import { geoplateformeWmts, IGN_ATTRIBUTION } from '../geoplateforme.ts';
import type { BasemapName, RasterOptions } from '../types.ts';

export { IGN_ATTRIBUTION };

const PLAN = 'GEOGRAPHICALGRIDSYSTEMS.PLANIGNV2';

/** The raster basemaps of the Géoplateforme (WMTS, Web Mercator), crediting the IGN */
export const BASEMAPS: Record<'ign-plan' | 'ign-plan-grey' | 'ign-ortho', RasterOptions> = {
  'ign-plan': geoplateformeWmts(PLAN),
  'ign-plan-grey': geoplateformeWmts(PLAN, { saturation: 0 }),
  'ign-ortho': geoplateformeWmts('ORTHOIMAGERY.ORTHOPHOTOS', { format: 'image/jpeg' }),
};

/** The former names of the Géoplateforme basemaps (before 0.5), and their names now: an error tells it */
export const RENAMED_BASEMAPS: Record<string, BasemapName> = {
  plan: 'ign-plan',
  'plan-grey': 'ign-plan-grey',
  ortho: 'ign-ortho',
  'plan-vector': 'ign-plan-vector',
  'plan-grey-vector': 'ign-plan-grey-vector',
};

/** Credits of OpenFreeMap's basemaps, as its TileJSON gives them */
export const OPENFREEMAP_ATTRIBUTION =
  '<a href="https://openfreemap.org">OpenFreeMap</a> <a href="https://www.openmaptiles.org/">© OpenMapTiles</a> ' +
  'Data from <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a>';

/** Credits of the Terrain Tiles of AWS's open data registry (Mapzen's Terrarium tiles) and of their sources */
export const TERRAIN_TILES_ATTRIBUTION =
  '<a href="https://github.com/tilezen/joerd/blob/master/docs/attribution.md">Terrain Tiles</a> (Mapzen, AWS)';

/** IGN hillshade, grey on transparent, zooms 6 to 17 */
export const HILLSHADE: RasterOptions = geoplateformeWmts('IGNF_ELEVATION.ELEVATIONGRIDCOVERAGE.SHADOW', {
  minZoom: 6,
  maxZoom: 17,
});

/** IGN terrain model (RGE ALTI and its fill-ins, France), raw altitudes in metres from the Géoplateforme WMS */
export const TERRAIN_URL =
  'https://data.geopf.fr/wms-r/wms?SERVICE=WMS&VERSION=1.3.0&REQUEST=GetMap&LAYERS=ELEVATION.ELEVATIONGRIDCOVERAGE.HIGHRES' +
  '&STYLES=&CRS=EPSG:3857&BBOX={bbox}&WIDTH=256&HEIGHT=256&FORMAT=image/x-bil;bits=32';

/** The relief of the Moon and Mars by default: NASA's Trek image services (ArcGIS), raw 32-bit altitudes in metres
 * asked for in the body's Mercator, with their credits. The Moon: LOLA's global model, 128 pixels per degree (237 m);
 * Mars: MOLA's, 128 and 64 pixels per degree merged (463 m), above the areoid */
export const BODY_TERRAINS = {
  moon: {
    url: 'https://trek.nasa.gov/moon/trekarcgis/rest/services/LRO_LOLA_DEM_Global_128ppd_v04/ImageServer',
    maxZoom: 8,
    attribution: '<a href="https://trek.nasa.gov/moon/">NASA Moon Trek</a>: LOLA terrain model, NASA/GSFC/MIT',
  },
  mars: {
    url: 'https://trek.nasa.gov/mars/trekarcgis/rest/services/mola128_mola64_merge_90Nto90S_SimpleC_clon0/ImageServer',
    maxZoom: 8,
    attribution: '<a href="https://trek.nasa.gov/mars/">NASA Mars Trek</a>: MOLA terrain model, NASA/JPL/GSFC',
  },
} as const;

/** An ArcGIS image service (a URL ending in `/ImageServer`) */
export const IMAGE_SERVER = /\/ImageServer\/?$/;

/**
 * The `exportImage` template of an ArcGIS image service for the relief: 256 × 256 raw 32-bit floats (`bsq`, followed
 * by its validity mask) of the box `{bbox}` in the Mercator of the body. On the Earth, Web Mercator (EPSG:3857);
 * elsewhere, a Mercator on the sphere of `radius` metres (Web Mercator's own formulas), given in WKT.
 */
export const imageServerTerrain = (url: string, earth: boolean, radius: number): string => {
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
};

/** The Earth's relief for `terrain: true`: the Terrain Tiles of AWS's open data registry, worldwide (SRTM, EU-DEM,
 * GMTED… merged by Mapzen), Terrarium PNGs of 256 px to level 14, altitudes above the mean sea level */
export const TERRAIN_TILES = {
  url: 'https://s3.amazonaws.com/elevation-tiles-prod/terrarium/{z}/{x}/{y}.png',
  maxZoom: 14,
} as const;

/** Sea color of each basemap, for the globe under the tiles (polar caps), RGB 0-1 */
export const SEA: Record<BasemapName, [r: number, g: number, b: number]> = {
  'osm-positron': [0.761, 0.784, 0.792],
  'osm-liberty': [0.62, 0.741, 1],
  'osm-bright': [0.682, 0.812, 0.886],
  'ign-plan': [0.667, 0.835, 0.914],
  'ign-plan-grey': [0.792, 0.792, 0.792],
  'ign-plan-vector': [0.667, 0.835, 0.914],
  'ign-plan-grey-vector': [0.792, 0.792, 0.792],
  'ign-ortho': [0.039, 0.231, 0.349],
  none: [0.85, 0.85, 0.83],
};
