/** Public types of the `isard` package */
import type { AttributionOptions } from './controls.ts';
import type { DecoderUrls } from './decoders.ts';
import type { Feature, Ramp, Size } from './features.ts';
import type { TileGrid } from './grid.ts';
import type { Layer } from './layers.ts';
import type { Camera, Engine } from './wasm/isard.js';

export type LonLat = [lon: number, lat: number];
export type Point = [x: number, y: number];

/**
 * The named basemaps. OpenFreeMap's vector styles, OpenStreetMap data worldwide, their labels above the other layers:
 * `osm-positron` (light grey, quiet: data over it reads first), `osm-liberty` and `osm-bright` (in colour). The
 * Géoplateforme's (IGN, France): rasters (`ign-plan`, `ign-plan-grey`, `ign-ortho`, the aerial photographs) or vector
 * tiles with their labels (`ign-plan-vector`, `ign-plan-grey-vector`, the PLAN.IGN styles).
 */
export type BasemapName =
  | 'osm-positron'
  | 'osm-liberty'
  | 'osm-bright'
  | 'ign-plan'
  | 'ign-plan-grey'
  | 'ign-ortho'
  | 'ign-plan-vector'
  | 'ign-plan-grey-vector'
  | 'none';

/**
 * @deprecated The former names of the Géoplateforme basemaps, still accepted (with a warning): the same names with the
 * `ign-` prefix (`'plan-grey'` is `'ign-plan-grey'`)
 */
export type LegacyBasemap = 'plan' | 'plan-grey' | 'plan-vector' | 'plan-grey-vector' | 'ortho';

/** A basemap by name (see `BasemapName`; the former names of `LegacyBasemap` are deprecated) */
export type Basemap = BasemapName | LegacyBasemap;

/**
 * The Earth's named terrain models. `'terrain-tiles'`: worldwide, the Terrain Tiles of AWS's open data registry
 * (SRTM, EU-DEM, GMTED… merged by Mapzen, 30 m in most places), the sea floor flattened at sea level. `'ign'`: France,
 * the IGN's RGE ALTI from the Géoplateforme (1 to 5 m), raised by the RAF20 geoid to heights above the ellipsoid.
 */
export type TerrainModel = 'terrain-tiles' | 'ign';

/** How tiles encode their altitudes: `'float32'`, raw little-endian floats (`image/x-bil;bits=32`, an ArcGIS raw
 * grid); `'terrarium'`, the colors of a PNG, `R × 256 + G + B / 256 - 32768`; `'mapbox'`, those of Terrain-RGB,
 * `(R × 65536 + G × 256 + B) / 10 - 10000` */
export type TerrainEncoding = 'float32' | 'terrarium' | 'mapbox';

/**
 * How tiles load and show. 'immediate' (default): everything visible loads, tiles show as soon as they arrive.
 * 'smooth': during flights the levels flown through are skipped, off-center tiles wait while the camera moves, tiles
 * fade in; fewer downloads, for slow networks or GPUs.
 */
export type LoadingMode = 'immediate' | 'smooth';

/** 'globe': a sphere at low zoom, blending into Web Mercator between zooms 5 and 7 */
export type Projection = 'globe' | 'mercator';

/** A body to draw the map on, other than the named ones (`CelestialBody`) */
export interface BodyOptions {
  /** Equatorial radius, metres */
  radius: number;
  /** Flattening of its ellipsoid, 0 for a sphere (default 0) */
  flattening?: number;
  /** An atmosphere: its halo around the globe, true for the Earth's light blue, or a CSS color (its alpha the
   * halo's strength) (default false) */
  atmosphere?: boolean | string;
  /** Color of the globe where no tile covers it (past the Web Mercator latitudes, 85°), a CSS color (default
   * `#808080`) */
  color?: string;
  /** Lowest and highest altitudes of its ground, metres around the ellipsoid: what its relief may raise into view
   * (default ±1 % of the radius) */
  relief?: [lowest: number, highest: number];
}

/**
 * The body the map is drawn on: 'earth' (WGS 84, the default), 'moon' (a sphere of 1 737 400 m, no atmosphere),
 * 'mars' (a sphere of 3 396 190 m, its equatorial radius, a thin dusty halo; planetocentric latitudes) or another
 * one. Zoom levels keep their meaning on screen (the Web Mercator tile pyramid), their metres follow the body:
 * altitudes, relief, buildings, 3D Tiles. Off the Earth, the map has none of the Earth's defaults: no basemap, no
 * credits, and space for its background; the Moon and Mars have NASA's relief for `terrain: true`.
 */
export type CelestialBody = 'earth' | 'moon' | 'mars' | BodyOptions;

export interface GeoMapOptions extends Partial<Camera> {
  /** The body the map is drawn on (default 'earth'); set at creation */
  body?: CelestialBody;
  /** Default 'globe' */
  projection?: Projection;
  /** Default 'immediate' */
  loading?: LoadingMode;
  /** The basemap (default 'osm-positron'; 'none' off the Earth) */
  basemap?: Basemap;
  /** Opacity of the IGN hillshade over the basemap (France, Géoplateforme), 0 for none (default 0) */
  hillshade?: number;
  /** Color behind the map (sky of tilted flat views), a CSS color (default `#f2f2f0`; black off the Earth) */
  background?: string;
  /** Space behind the globe: stars and atmosphere halo, fading out as the globe turns flat (default true) */
  sky?: boolean;
  /** Relief under the basemap: true for the body's terrain model (on the Earth the worldwide Terrain Tiles; NASA's on
   * the Moon and Mars: LOLA, MOLA), one of the Earth's models by name (`'ign'`: France, RGE ALTI), or its options
   * (default false; on another body, its `url` is required). Network layers are draped on it */
  terrain?: boolean | TerrainModel | TerrainOptions;
  minZoom?: number;
  maxZoom?: number;
  /** Highest pitch, degrees, up to 85 (default 70) */
  maxPitch?: number;
  /** Sky over the plane, seen when the camera is tilted towards the horizon, and the fog the ground fades into
   * (MapLibre's `setSky`); default none: the background color */
  atmosphere?: AtmosphereOptions;
  /** Mouse, touch and keyboard controls (default true) */
  interactive?: boolean;
  /** Keyboard controls once the map has the focus (it is reached with Tab): arrows pan, Shift + arrows rotate and
   * tilt, + and - zoom (Shift: two levels), 0 faces north and levels the view (default true; none when not
   * `interactive`) */
  keyboard?: boolean;
  /** Accessible name of the map, read by screen readers (default 'Map') */
  ariaLabel?: string;
  /** Where to load the engine with 3D Tiles from (`isard_bg.wasm`), when a bundler does not resolve the default URL.
   * Given without `coreWasmUrl`, maps load this engine from the start */
  wasmUrl?: string | URL;
  /** Where to load the engine without 3D Tiles from (`isard_core_bg.wasm`, a third smaller), when a bundler does not
   * resolve the default URL. Maps load it first, and the engine with 3D Tiles (`wasmUrl`) for their first 3D Tiles
   * layer only */
  coreWasmUrl?: string | URL;
  /** Where the decoding workers load the point cloud module from (`isard_points_bg.wasm`, the LAZ decoder of COPC),
   * when a bundler does not resolve the default URL. Loaded with the first point cloud only */
  pointsWasmUrl?: string | URL;
  /** Load the engine with 3D Tiles from the start (default false: it is loaded when the first 3D Tiles layer or glTF
   * model is added, the map then moving its camera and layers onto it, a stall of about a frame or two). For a map that shows 3D Tiles
   * at once: one engine downloaded instead of two. `'preload'`: the map starts on the engine without 3D Tiles and
   * downloads and compiles the other once idle, without running it: its first 3D Tiles layer does not wait for the
   * download (for a map that will show 3D Tiles later, on demand) */
  tiles3d?: boolean | 'preload';
  /** Where to load the vector basemap worker from, when a bundler does not resolve the default URL */
  basemapWorkerUrl?: string | URL;
  /** Texture size of the vector basemap's tiles, pixels (default 512: lines and edges sharp at every zoom on a screen
   * of pixel ratio 1). 256: a quarter of the drawing in the workers, of the uploads and of the GPU memory, for lines
   * and edges a little softer between two zoom levels (the tiles are shown 181 to 362 CSS pixels wide) */
  basemapTileSize?: number;
  /** Glyphs of the symbol layers: a URL with `{fontstack}` and `{range}` (MapLibre's glyph format; default
   * OpenFreeMap's fonts, Noto Sans). A vector basemap's labels take the glyphs of its style */
  glyphs?: string;
  /** Workers decoding 3D Tiles off the main thread: a count, or false to decode on the main thread (default: cores - 1,
   * 1 to 4). False (or 0) also decodes the PNG relief and reprojects the equirectangular rasters on the main thread,
   * else done by the vector basemap's workers */
  workers?: number | false;
  /** Where to load the decoding worker from, when a bundler does not resolve the default URL */
  workerUrl?: string | URL;
  /** Folders of the Draco and Basis Universal decoders, loaded by the workers on first use (default: the official
   * builds on gstatic.com and jsDelivr; self-host them for offline or locked-down networks). They run through
   * `new Function`: a Content Security Policy needs `'unsafe-eval'` (see `DecoderUrls`) */
  decoders?: Partial<DecoderUrls>;
  /** Credits at the bottom right: false for none, or its options (default true: the credits of the basemap,
   * hillshade and relief in use, compact on maps narrower than 640 px) */
  attributionControl?: boolean | AttributionOptions;
}

export interface TerrainOptions {
  /** One of the Earth's named models (default `'terrain-tiles'` on the Earth); unused with a `url` */
  model?: TerrainModel;
  /** Altitudes of 256 × 256 tiles in `encoding`: a WMS GetMap URL with {bbox} (EPSG:3857 on the Earth, the body's
   * Mercator metres elsewhere: the sphere of its radius), a tile URL with {z}, {x} and {y}, or an ArcGIS image service
   * (a URL ending in `/ImageServer`), asked for raw altitudes in the body's Mercator (default: the `model`'s; on the
   * Moon and Mars, NASA's Trek LOLA and MOLA models) */
  url?: string;
  /** How the tiles of `url` encode their altitudes (default `'float32'`); `'terrarium'` and `'mapbox'` take a URL with
   * {z}, {x} and {y} */
  encoding?: TerrainEncoding;
  /** Vertical exaggeration, 1 for the true relief (default 1) */
  exaggeration?: number;
  /** Finest level of the relief, up to 15 (default 15; 14 for the Terrain Tiles, 8 for the Moon's and Mars's models). It
   * starts at level 6 on the Earth, coarser on bodies of a larger relief for their size (3 on the Moon and Mars) */
  maxZoom?: number;
  /** Altitudes over mainland France raised by the height of the RAF20 geoid (the French one, 0 away from France), to
   * heights above the ellipsoid (those of 3D Tiles); false keeps the altitudes as served (NGF-IGN69 for the IGN's)
   * (default true for the `'ign'` model, false otherwise) */
  geoid?: boolean;
}

/** Sky, horizon and fog over the plane, with the names and meanings of MapLibre's `setSky` */
export interface AtmosphereOptions {
  /** A CSS color (default `#9cc7ec`) */
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

/**
 * Tile grid of a raster's URL. 'mercator': Web Mercator (XYZ, WMTS PM), the engine's own. 'equirectangular':
 * longitudes and latitudes (Plate Carrée, EPSG:4326-like), two 256 px tiles side by side at level 0, rows from the
 * north (NASA's Trek WMTS, WorldCRS84Quad). A `TileGrid`: the tile matrices of a WMTS in any CRS (Lambert-93…), or a
 * WMS asked for each tile's box in its CRS. The last two are reprojected into Web Mercator tiles by the map's workers
 * (in the page with `workers: false`).
 */
export type RasterGrid = 'mercator' | 'equirectangular' | TileGrid;

/** What `RasterLayer.setStyle` changes */
export interface RasterStyle {
  opacity: number;
  /** 0 = greyscale, 1 = original colors */
  saturation: number;
}

export interface RasterOptions {
  /** Tile URL with {z}, {x} and {y} (256 px tiles); unused with a `loader` */
  url: string;
  /** Tile grid of `url` (default 'mercator'); with 'equirectangular', `minZoom` and `maxZoom` are levels of that
   * grid; with a `TileGrid`, they are map zooms, by default those its matrices span */
  grid?: RasterGrid;
  minZoom?: number;
  maxZoom?: number;
  opacity?: number;
  /** 0 = greyscale, 1 = original colors */
  saturation?: number;
  /** Side of the tiles' textures, pixels (default 256): more for sharper tiles drawn by a `loader` */
  size?: number;
  /** Draws each tile instead of fetching `url`: an image of `size` × `size`, null for nothing there; a rejection is
   * retried like a network error */
  loader?: (z: number, x: number, y: number, signal: AbortSignal) => Promise<ImageBitmap | null>;
  /** Credits of the tiles, HTML: shown by the attribution control while the layer is on the map */
  attribution?: string;
}

export interface NetworkStyle {
  /** Line width, CSS pixels, or zoom stops `[[zoom, width], …]` (default 2) */
  width: Size;
  /** Values above are drawn as unreached (default 1800) */
  max: number;
  /** Darker line every `contour` value units, 0 for none (default 0) */
  contour: number;
  opacity: number;
  /** Width of the halo drawn under the lines, CSS pixels or zoom stops, 0 for none (default 0): wide and faint, the
   * reached streets add up into a heat map that fills the blocks */
  halo: Size;
  /** Opacity of the halo at the middle of a line, fading quadratically to its sides (default 0.11) */
  haloOpacity: number;
  /** Colors from 0 to `max`: `[[fraction of max, color], …]` (default green to red) */
  ramp: Ramp | undefined;
  /** Color of the parts beyond `max` or unreached (default `rgba(140, 140, 140, 0.25)`) */
  unreachedColor: string;
  /** Share of `max` over which the colors fade out before it, 0 to 1 (default 0) */
  fade: number;
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
  /** Width factor of each edge (default 1): rail lines wider than streets */
  edgeWidths?: Float32Array;
}

export interface Tiles3dStyle {
  /** Screen-space error (CSS pixels) above which a tile is refined: 16 by default, lower is finer and heavier */
  maxScreenSpaceError: number;
  opacity: number;
  /** Size of the points of point cloud tiles (`.pnts`, glTF points), CSS pixels: 2 by default */
  pointSize?: number;
}

export interface Tiles3dOptions extends Partial<Tiles3dStyle> {
  /** URL of a `tileset.json` (3D Tiles 1.0 b3dm and pnts, 1.1 glb meshes and points, external tilesets) */
  url: string;
}

export interface MapPointerEvent {
  point: Point;
  /** Undefined above the horizon */
  lonLat: LonLat | undefined;
}

/** What 'hoverfeature' found under the pointer */
export interface MapHoverFeatureEvent extends MapPointerEvent {
  /** The feature under the pointer (see `GeoMap.pickFeature`), undefined if none */
  hit: FeatureHit | undefined;
}

/** A picked feature: its layer handle, its index there and, for line, circle and fill layers, the feature itself */
export interface FeatureHit {
  layer: Layer;
  index: number;
  feature: Feature | undefined;
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
  /** What is under the pointer, picked without stalling the GPU: a frame or two after 'hover', only while listened to
   * (radius and layers in `GeoMap.hoverPicking`) */
  hoverfeature: MapHoverFeatureEvent;
  /** Click without drag */
  click: MapPointerEvent;
  /** A frame was drawn; duration of the engine call in milliseconds */
  render: { ms: number };
  /** Everything in view is loaded and drawn, nothing moves: the end of a load, once after each change (as MapLibre's
   * `idle`). Tiles requested by the first frame count, so it never fires before they arrive. */
  idle: Record<string, never>;
  /** The pointer left the canvas (a mouse going out of the map, or onto a control or marker) */
  leave: Record<string, never>;
}

/** What a layer handle needs from its map */
export interface LayerHost {
  readonly engine: Engine;
  requestRender(): void;
  /** Forgets a removed layer */
  detach(layer: object): void;
  /** Moves a layer under `before`, or on top */
  moveLayer(layer: object, before: object | undefined): void;
  /** A layer created again in the engine (new id): put at the place of its old id, which is removed */
  replaceLayer(layer: object, old: number): void;
}
