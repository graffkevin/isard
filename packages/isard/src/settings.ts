/**
 * The settings of a map's engine from its options: the body, its halo, its relief, the sky. Kept by the map and
 * replayed on a new engine (after a context loss, or on the engine with 3D Tiles).
 */
import {
  BODY_TERRAINS,
  IGN_ATTRIBUTION,
  IMAGE_SERVER,
  imageServerTerrain,
  TERRAIN_TILES,
  TERRAIN_TILES_ATTRIBUTION,
  TERRAIN_URL,
} from './basemap/basemaps.ts';
import type { PngEncoding } from './basemap/terrain.ts';
import type {
  AtmosphereOptions,
  BodyOptions,
  CelestialBody,
  LoadingMode,
  TerrainModel,
  TerrainOptions,
} from './types.ts';
import { parseCssColor } from './util/color.ts';

/** Decodes the relief tiles of a URL for the engine: 256 × 256 altitudes, null where there is no tile */
export type TerrainLoader = (z: number, x: number, y: number, signal: AbortSignal) => Promise<Float32Array | null>;

/** Settings replayed on a new engine after a WebGL context loss */
export interface Settings {
  body: Required<BodyOptions>;
  zoomRange: [min: number, max: number];
  maxPitch: number;
  globe: boolean;
  sky: boolean;
  atmosphere: Float32Array | undefined;
  terrain:
    | {
        url: string;
        exaggeration: number;
        maxZoom: number;
        geoid: boolean;
        /** Decodes the tiles of `url` for the engine (an encoding other than raw floats) */
        loader?: TerrainLoader;
        /** Credits of the model */
        attribution?: string;
      }
    | undefined;
  loading: LoadingMode;
  background: [r: number, g: number, b: number];
  globeColor: [r: number, g: number, b: number];
}

/** The named bodies; a body's `color` is that of its globe under the tiles (the Earth's follows its basemap), its
 * `relief` the range of its ground (the Earth's dry land, from the Dead Sea to Everest). Mars: the sphere of its
 * equatorial radius (its data take planetocentric latitudes), a faint butterscotch halo of dust */
const BODIES: Record<Exclude<CelestialBody, BodyOptions>, Required<BodyOptions>> = {
  earth: { radius: 6378137, flattening: 1 / 298.257223563, atmosphere: true, color: '#cacaca', relief: [-450, 8900] },
  moon: { radius: 1737400, flattening: 0, atmosphere: false, color: '#77756f', relief: [-9200, 10800] },
  mars: {
    radius: 3396190,
    flattening: 0,
    atmosphere: 'rgba(232, 168, 120, 0.6)',
    color: '#b5714a',
    relief: [-8300, 21300],
  },
};

/** The Earth's halo, RGB 0-1 and its strength: what `atmosphere: true` draws */
const EARTH_HALO = Float32Array.of(0.42, 0.66, 1, 1);

/** A body from the option, its defaults filled in */
export const bodySettings = (body: CelestialBody | undefined): Required<BodyOptions> => {
  if (body === undefined || typeof body === 'string') return BODIES[body ?? 'earth'];
  const relief: [number, number] = [-0.01 * body.radius, 0.01 * body.radius];
  return { flattening: 0, atmosphere: false, color: '#808080', relief, ...body };
};

/** The halo of an atmosphere for the engine (RGB 0-1 and its strength, the color's alpha): undefined for none */
export const haloSettings = (atmosphere: boolean | string): Float32Array | undefined => {
  if (atmosphere === true) return EARTH_HALO;
  const color = atmosphere === false ? undefined : parseCssColor(atmosphere);
  return color && Float32Array.from(color);
};

/** Relief settings from the option: undefined when off. On the Earth the Terrain Tiles by default (worldwide,
 * Terrarium PNGs decoded by `terrainLoader`) or the IGN terrain model (France, raised by the geoid); NASA's on the Moon
 * and Mars; an ArcGIS image service asked for in the body's Mercator. `terrainLoader`: the loader of PNG tiles in an
 * encoding, the same for the same tiles (the engine keeps its tiles when only the exaggeration changes) */
/** The relief options of the option on */
const terrainOptions = (terrain: true | TerrainModel | TerrainOptions): TerrainOptions => {
  if (typeof terrain === 'string') return { model: terrain };
  return terrain === true ? {} : terrain;
};

export const terrainSettings = (
  terrain: boolean | TerrainModel | TerrainOptions | undefined,
  body: CelestialBody | undefined,
  radius: number,
  terrainLoader: (url: string, encoding: PngEncoding, floor?: number) => TerrainLoader,
): Settings['terrain'] => {
  if (!terrain) return undefined;
  const options = terrainOptions(terrain);
  const earth = (body ?? 'earth') === 'earth';
  const exaggeration = options.exaggeration ?? 1;
  if (options.url === undefined && earth) {
    if (options.model === 'ign') {
      const geoid = options.geoid ?? true;
      return { url: TERRAIN_URL, exaggeration, maxZoom: options.maxZoom ?? 15, geoid, attribution: IGN_ATTRIBUTION };
    }
    return {
      url: TERRAIN_TILES.url,
      exaggeration,
      maxZoom: options.maxZoom ?? TERRAIN_TILES.maxZoom,
      geoid: options.geoid ?? false,
      // The sea floor of the model at sea level: the oceans stay flat under the basemap's water
      loader: terrainLoader(TERRAIN_TILES.url, 'terrarium', 0),
      attribution: TERRAIN_TILES_ATTRIBUTION,
    };
  }
  const preset = body === 'moon' || body === 'mars' ? BODY_TERRAINS[body] : undefined;
  const url = options.url ?? preset?.url;
  if (url === undefined) throw new Error('isard: the relief of this body needs a url');
  const encoding = options.encoding ?? 'float32';
  // PNG encodings are fetched tile by tile: a {bbox} or an image service would be fetched as it is
  if (
    encoding !== 'float32' &&
    (IMAGE_SERVER.test(url) || !/\{z\}/.test(url) || !/\{x\}/.test(url) || !/\{y\}/.test(url))
  ) {
    throw new Error(`isard: the '${encoding}' terrain encoding takes a tile URL with {z}, {x} and {y} (${url})`);
  }
  return {
    url: IMAGE_SERVER.test(url) ? imageServerTerrain(url, earth, radius) : url,
    exaggeration,
    maxZoom: options.maxZoom ?? (options.url === undefined && preset ? preset.maxZoom : 15),
    geoid: options.geoid ?? false,
    ...(encoding === 'float32' ? {} : { loader: terrainLoader(url, encoding) }),
    // NASA's credits for its models
    ...(options.url === undefined && preset ? { attribution: preset.attribution } : {}),
  };
};

/** Sky settings for the engine: colors (RGB 0-1) and blends, MapLibre's defaults */
export const atmosphereSettings = (options: AtmosphereOptions | undefined): Float32Array | undefined => {
  if (!options) return undefined;
  return Float32Array.from([
    ...rgb(options.skyColor ?? '#9cc7ec'),
    ...rgb(options.horizonColor ?? '#e8f1f8'),
    ...rgb(options.fogColor ?? '#f2f5f8'),
    options.skyHorizonBlend ?? 0.6,
    options.horizonFogBlend ?? 0.7,
    options.fogGroundBlend ?? 0.4,
  ]);
};

/** A CSS color to RGB 0-1 (its alpha dropped); black if unknown */
export const rgb = (color: string): [r: number, g: number, b: number] => {
  const [r, g, b] = parseCssColor(color) ?? [0, 0, 0, 1];
  return [r, g, b];
};
