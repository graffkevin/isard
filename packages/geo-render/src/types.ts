/** Public types of the `geo-render` package */
import type { Camera, Engine } from './wasm/geo_render.js';

export type LonLat = [lon: number, lat: number];
export type Point = [x: number, y: number];

export type Basemap = 'plan' | 'plan-grey' | 'ortho' | 'none';

/** 'globe': a sphere at low zoom, blending into Web Mercator between zooms 5 and 7 */
export type Projection = 'globe' | 'mercator';

export interface GeoMapOptions extends Partial<Camera> {
  /** Default 'globe' */
  projection?: Projection;
  /** Géoplateforme basemap (default 'plan-grey') */
  basemap?: Basemap;
  /** Opacity of the IGN hillshade over the basemap, 0 for none (default 0.4) */
  hillshade?: number;
  /** Color behind the map (sky of tilted flat views), `#rrggbb` (default `#f2f2f0`) */
  background?: string;
  /** Space behind the globe: stars and atmosphere halo, fading out as the globe turns flat (default true) */
  sky?: boolean;
  minZoom?: number;
  maxZoom?: number;
  /** Mouse and touch controls (default true) */
  interactive?: boolean;
  /** Where to load the engine from, when a bundler does not resolve the default URL */
  wasmUrl?: string | URL;
  /** Workers decoding 3D Tiles off the main thread: a count, or false to decode on the main thread (default: cores - 1,
   * 1 to 4) */
  workers?: number | false;
  /** Where to load the decoding worker from, when a bundler does not resolve the default URL */
  workerUrl?: string | URL;
}

export interface RasterOptions {
  /** Tile URL with {z}, {x} and {y} (Web Mercator, 256 px tiles) */
  url: string;
  minZoom?: number;
  maxZoom?: number;
  opacity?: number;
  /** 0 = greyscale, 1 = original colors */
  saturation?: number;
}

export interface NetworkStyle {
  /** Line width, CSS pixels (default 2) */
  width: number;
  /** Values above are drawn as unreached (default 1800) */
  max: number;
  /** Darker line every `contour` value units, 0 for none (default 0) */
  contour: number;
  opacity: number;
}

export interface NetworkOptions extends Partial<NetworkStyle> {
  /** Node coordinates in degrees: lon0, lat0, lon1, lat1… */
  nodes: Float64Array;
  /** Segments as node index pairs: a0, b0, a1, b1… */
  edges: Uint32Array;
}

export interface Tiles3dStyle {
  /** Screen-space error (CSS pixels) above which a tile is refined: 16 by default, lower is finer and heavier */
  maxScreenSpaceError: number;
  opacity: number;
}

export interface Tiles3dOptions extends Partial<Tiles3dStyle> {
  /** URL of a `tileset.json` (3D Tiles 1.0 b3dm or 1.1 glb meshes, external tilesets) */
  url: string;
}

export interface MapPointerEvent {
  point: Point;
  /** Undefined above the horizon */
  lonLat: LonLat | undefined;
}

export interface MapEvents {
  /** The camera moved */
  move: Camera;
  /** The pointer moved over the map (at most once per frame, not while dragging) */
  hover: MapPointerEvent;
  /** Click without drag */
  click: MapPointerEvent;
  /** A frame was drawn; duration of the engine call in milliseconds */
  render: { ms: number };
}

/** What a layer handle needs from its map */
export interface LayerHost {
  readonly engine: Engine;
  requestRender(): void;
  /** Forgets a removed layer */
  detach(layer: object): void;
}
