/** Public types of the `isard` package */
import type { DecoderUrls } from './decoders.ts';
import type { Camera, Engine } from './wasm/isard.js';

export type LonLat = [lon: number, lat: number];
export type Point = [x: number, y: number];

export type Basemap = 'plan' | 'plan-grey' | 'ortho' | 'none';

/**
 * How tiles load and show. 'immediate' (default): everything visible loads, tiles show as soon as they arrive.
 * 'smooth': during flights the levels flown through are skipped, off-center tiles wait while the camera moves, tiles
 * fade in; fewer downloads, for slow networks or GPUs.
 */
export type LoadingMode = 'immediate' | 'smooth';

/** 'globe': a sphere at low zoom, blending into Web Mercator between zooms 5 and 7 */
export type Projection = 'globe' | 'mercator';

export interface GeoMapOptions extends Partial<Camera> {
  /** Default 'globe' */
  projection?: Projection;
  /** Default 'immediate' */
  loading?: LoadingMode;
  /** Géoplateforme basemap (default 'plan-grey') */
  basemap?: Basemap;
  /** Opacity of the IGN hillshade over the basemap, 0 for none (default 0.4) */
  hillshade?: number;
  /** Color behind the map (sky of tilted flat views), `#rrggbb` (default `#f2f2f0`) */
  background?: string;
  /** Space behind the globe: stars and atmosphere halo, fading out as the globe turns flat (default true) */
  sky?: boolean;
  /** Relief under the basemap: true for the IGN terrain model (Géoplateforme, RGE ALTI), or its options (default
   * false). Network layers are draped on it */
  terrain?: boolean | TerrainOptions;
  minZoom?: number;
  maxZoom?: number;
  /** Highest pitch, degrees, up to 85 (default 70) */
  maxPitch?: number;
  /** Sky over the plane, seen when the camera is tilted towards the horizon, and the fog the ground fades into
   * (MapLibre's `setSky`); default none: the background color */
  atmosphere?: AtmosphereOptions;
  /** Mouse and touch controls (default true) */
  interactive?: boolean;
  /** Where to load the engine from, when a bundler does not resolve the default URL */
  wasmUrl?: string | URL;
  /** Workers decoding 3D Tiles off the main thread: a count, or false to decode on the main thread (default: cores - 1,
   * 1 to 4) */
  workers?: number | false;
  /** Where to load the decoding worker from, when a bundler does not resolve the default URL */
  workerUrl?: string | URL;
  /** Folders of the Draco and Basis Universal decoders, loaded by the workers on first use (default: the official
   * builds on gstatic.com and jsDelivr; self-host them for offline or locked-down networks) */
  decoders?: Partial<DecoderUrls>;
}

export interface TerrainOptions {
  /** Raw altitudes, 256 × 256 `image/x-bil;bits=32`: a WMS GetMap URL with {bbox} (EPSG:3857) or a tile URL with {z},
   * {x} and {y} (default: the Géoplateforme WMS, ELEVATION.ELEVATIONGRIDCOVERAGE.HIGHRES). Altitudes over mainland
   * France (NGF-IGN69) are raised by the RAF20 geoid height to meet the ellipsoidal heights of 3D Tiles */
  url?: string;
  /** Vertical exaggeration, 1 for the true relief (default 1) */
  exaggeration?: number;
  /** Finest level of the relief, 6 to 15 (default 15) */
  maxZoom?: number;
  /** Altitudes raised by the RAF20 geoid height, to heights above the ellipsoid (those of 3D Tiles); false keeps the
   * altitudes as served (NGF-IGN69 over France) (default true) */
  geoid?: boolean;
}

/** Sky, horizon and fog over the plane, with the names and meanings of MapLibre's `setSky` */
export interface AtmosphereOptions {
  /** `#rrggbb` (default `#9cc7ec`) */
  skyColor?: string;
  /** (default `#e8f1f8`) */
  horizonColor?: string;
  /** (default `#f2f5f8`) */
  fogColor?: string;
  /** Blend from the horizon to the sky color, share of the field of view above the horizon, 0-1 (default 0.6) */
  skyHorizonBlend?: number;
  /** Blend from the horizon to the fog color below the horizon, 0-1 (default 0.7) */
  horizonFogBlend?: number;
  /** Share of the distance to the far plane over which the ground fades into the fog, 0-1 (default 0.4) */
  fogGroundBlend?: number;
}

/** Animation of a camera move */
export interface AnimationOptions {
  /** Milliseconds */
  duration?: number;
  /** Progress over time, both 0 to 1 */
  easing?: (t: number) => number;
}

export interface FlyToOptions extends AnimationOptions {
  /** How far the flight zooms out on its way, as MapLibre's `curve` (default 1.42) */
  curve?: number;
  /** Average speed in screens per second when `duration` is not given (default 1.2) */
  speed?: number;
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
  /** Width of the halo drawn under the lines, CSS pixels, 0 for none (default 0): wide and faint, the reached
   * streets add up into a heat map that fills the blocks */
  halo: number;
  /** Opacity of the halo at the middle of a line, fading quadratically to its sides (default 0.11) */
  haloOpacity: number;
}

export interface NetworkOptions extends Partial<NetworkStyle> {
  /** Node coordinates in degrees: lon0, lat0, lon1, lat1… */
  nodes: Float64Array;
  /** Edges as node index pairs: a0, b0, a1, b1… */
  edges: Uint32Array;
  /** Line of each edge, from A to B: the points of edge e are `points[start[e] .. start[e + 1]]` (lon, lat pairs,
   * counted in points); with fewer than two points an edge is straight. Default: all straight. */
  lines?: { start: Uint32Array; points: Float64Array };
  /** Per edge, the cost of travelling it from A to B then from B to A, in value units (Infinity: closed that way;
   * closed both ways: not drawn). A point along an edge then takes the quickest way there, min(value(A) + cost from
   * A, value(B) + cost from B), as a travel time does. Without costs, values are interpolated along the edges. */
  costs?: Float32Array;
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
  /** The camera started moving (a gesture, an animation or `setCamera`) */
  movestart: Camera;
  /** The camera moved */
  move: Camera;
  /** The camera stopped: no gesture held, no animation running */
  moveend: Camera;
  /** The map changed size (CSS pixels) */
  resize: { width: number; height: number };
  /** The pointer moved over the map (at most once per frame, not while dragging) */
  hover: MapPointerEvent;
  /** Click without drag */
  click: MapPointerEvent;
  /** A frame was drawn; duration of the engine call in milliseconds */
  render: { ms: number };
  /** Everything in view is loaded and drawn, nothing moves: the end of a load, once after each change (as MapLibre's
   * `idle`). Tiles requested by the first frame count, so it never fires before they arrive. */
  idle: Record<string, never>;
}

/** What a layer handle needs from its map */
export interface LayerHost {
  readonly engine: Engine;
  requestRender(): void;
  /** Forgets a removed layer */
  detach(layer: object): void;
}
