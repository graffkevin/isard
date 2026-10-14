/**
 * geo-render: WebGL2 map engine for large, interactive data layers on Géoplateforme basemaps.
 *
 * ```ts
 * import { GeoMap } from 'geo-render';
 * const map = await GeoMap.create(document.getElementById('map')!, { center: [2.4246, 48.8457], zoom: 13 });
 * const network = map.addNetwork({ nodes, edges, max: 1800, contour: 300 });
 * map.on('hover', ({ lonLat }) => network.setValues(travelTimes(lonLat)));
 * ```
 *
 * Frames are drawn on demand only: an idle map costs nothing. Hover events are coalesced to one per frame, so the
 * values computed for a mouse position are drawn in the same frame.
 */
import init, { Engine } from './wasm/geo_render.js';
import { Interaction } from './interaction.ts';
import { TileLoader } from './loader.ts';

export type LonLat = [lon: number, lat: number];
export type Point = [x: number, y: number];

export interface Camera {
  center: LonLat;
  zoom: number;
  /** Degrees from the vertical, 0 to 70 */
  pitch: number;
  /** Degrees clockwise from the north */
  bearing: number;
}

export type Basemap = 'plan' | 'plan-grey' | 'ortho' | 'none';

/** 'globe': a sphere at low zoom, blending into Web Mercator between zooms 5 and 7 */
export type Projection = 'globe' | 'mercator';

export interface LayerStats {
  id: number;
  /** Tiles drawn in the last frame (stand-ins included) */
  drawn: number;
  /** Tiles held on the GPU */
  loaded: number;
  /** Requests in flight */
  inFlight: number;
  /** Arrived tiles waiting for their upload */
  queued: number;
  /** Drawn with an ancestor's texture: the tile itself is not loaded yet */
  standIns: number;
  /** Waiting before a new attempt after errors */
  cooling: number;
  /** Not served (404), covered by an ancestor */
  missing: number;
}

export interface GeoMapOptions extends Partial<Camera> {
  /** Default 'globe' */
  projection?: Projection;
  /** Géoplateforme basemap (default 'plan-grey') */
  basemap?: Basemap;
  /** Opacity of the IGN hillshade over the basemap, 0 for none (default 0.4) */
  hillshade?: number;
  minZoom?: number;
  maxZoom?: number;
  /** Mouse and touch controls (default true) */
  interactive?: boolean;
  /** Where to load the engine from, when a bundler does not resolve the default URL */
  wasmUrl?: string | URL;
  /** Workers decoding 3D Tiles off the main thread: a number, or false to decode on the main thread (default:
   * cores - 1, 1 to 4) */
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
  width?: number;
  /** Values above are drawn as unreached (default 1800) */
  max?: number;
  /** Darker line every `contour` value units, 0 for none (default 0) */
  contour?: number;
  opacity?: number;
}

export interface NetworkOptions extends NetworkStyle {
  /** Node coordinates in degrees: lon0, lat0, lon1, lat1… */
  nodes: Float64Array;
  /** Segments as node index pairs: a0, b0, a1, b1… */
  edges: Uint32Array;
}

export interface Tiles3dOptions {
  /** URL of a `tileset.json` (3D Tiles 1.0 b3dm or 1.1 glb meshes, external tilesets) */
  url: string;
  /** Screen-space error (CSS pixels) above which a tile is refined: 16 by default, lower is finer and heavier */
  maxScreenSpaceError?: number;
  opacity?: number;
}

export interface Tiles3dStats {
  /** Tiles drawn in the last frame */
  drawn: number;
  /** Meshes held on the GPU */
  loaded: number;
  inFlight: number;
  /** GPU memory of the meshes, MB */
  gpuMb: number;
  /** Tiles known in the tree (external tilesets included once loaded) */
  tiles: number;
}

export interface PointerEvent {
  point: Point;
  /** Undefined above the horizon */
  lonLat?: LonLat;
}

interface Events {
  /** The camera moved */
  move: Camera;
  /** The pointer moved over the map (at most once per frame, not while dragging) */
  hover: PointerEvent;
  /** Click without drag */
  click: PointerEvent;
  /** A frame was drawn; duration of the engine call in milliseconds */
  render: { ms: number };
}

const GPF = 'https://data.geopf.fr/wmts?SERVICE=WMTS&REQUEST=GetTile&VERSION=1.0.0&STYLE=normal&TILEMATRIXSET=PM' +
  '&TILEMATRIX={z}&TILEROW={y}&TILECOL={x}';
const BASEMAPS: Record<Exclude<Basemap, 'none'>, RasterOptions> = {
  plan: { url: `${GPF}&LAYER=GEOGRAPHICALGRIDSYSTEMS.PLANIGNV2&FORMAT=image/png`, maxZoom: 19 },
  'plan-grey': { url: `${GPF}&LAYER=GEOGRAPHICALGRIDSYSTEMS.PLANIGNV2&FORMAT=image/png`, maxZoom: 19, saturation: 0 },
  ortho: { url: `${GPF}&LAYER=ORTHOIMAGERY.ORTHOPHOTOS&FORMAT=image/jpeg`, maxZoom: 19 },
};
/** Sea color of each basemap, for the globe under the tiles (polar caps), RGB 0-1 */
const SEA: Record<Basemap, [number, number, number]> = {
  plan: [0.667, 0.835, 0.914],
  'plan-grey': [0.792, 0.792, 0.792],
  ortho: [0.039, 0.231, 0.349],
  none: [0.85, 0.85, 0.83],
};
const HILLSHADE = `${GPF}&LAYER=IGNF_ELEVATION.ELEVATIONGRIDCOVERAGE.SHADOW&FORMAT=image/png`;

/** The engine, compiled once per page and shared with the decoding workers */
let compiled: Promise<WebAssembly.Module> | undefined;

async function compile(url: string | URL): Promise<WebAssembly.Module> {
  try {
    return await WebAssembly.compileStreaming(fetch(url));
  } catch {
    // Servers that do not send application/wasm
    return WebAssembly.compile(await (await fetch(url)).arrayBuffer());
  }
}

export class GeoMap {
  readonly canvas: HTMLCanvasElement;
  /** @internal */
  readonly engine: Engine;
  private readonly listeners = new Map<keyof Events, Set<(e: never) => void>>();
  private readonly resizeObserver: ResizeObserver;
  private readonly interaction?: Interaction;
  private frameRequest = 0;
  private lastCamera = '';
  private animation?: (now: number) => boolean;
  private hoverPoint?: Point;
  private loader?: TileLoader;
  private basemap?: RasterLayer;
  private hillshade?: RasterLayer;
  private inFrame = false;

  /** Loads the engine (once per page) and creates a map filling `container` */
  static async create(container: HTMLElement, options: GeoMapOptions = {}): Promise<GeoMap> {
    compiled ??= compile(options.wasmUrl ?? new URL('./geo_render_bg.wasm', import.meta.url)).then(async (module) => {
      await init({ module_or_path: module });
      return module;
    });
    const module = await compiled;
    const map = new GeoMap(container, options);
    const workers = options.workers ?? Math.min(4, Math.max(1, (navigator.hardwareConcurrency || 2) - 1));
    if (workers && typeof Worker !== 'undefined') {
      map.loader = new TileLoader(module, options.workerUrl ?? new URL('./decoder.worker.js', import.meta.url), workers);
      map.engine.setTileLoader(map.loader);
    }
    return map;
  }

  private constructor(container: HTMLElement, options: GeoMapOptions) {
    this.canvas = document.createElement('canvas');
    Object.assign(this.canvas.style, { display: 'block', width: '100%', height: '100%', touchAction: 'none' });
    container.append(this.canvas);
    this.engine = new Engine(this.canvas, () => this.requestRender());
    this.engine.setZoomRange(options.minZoom ?? 0, options.maxZoom ?? 22);
    this.engine.setGlobe((options.projection ?? 'globe') === 'globe');
    this.setCamera({ center: [2.4246, 48.8457], zoom: 13, pitch: 0, bearing: 0, ...options });
    this.setBasemap(options.basemap ?? 'plan-grey', options.hillshade ?? 0.4);
    this.resizeObserver = new ResizeObserver(() => this.resize());
    this.resizeObserver.observe(container);
    this.resize();
    if (options.interactive ?? true) this.interaction = new Interaction(this);
  }

  // ── Events ──

  on<K extends keyof Events>(type: K, listener: (event: Events[K]) => void): () => void {
    let set = this.listeners.get(type);
    if (!set) this.listeners.set(type, (set = new Set()));
    set.add(listener as (e: never) => void);
    return () => set.delete(listener as (e: never) => void);
  }

  /** @internal */
  emit<K extends keyof Events>(type: K, event: Events[K]): void {
    for (const listener of this.listeners.get(type) ?? []) (listener as (e: Events[K]) => void)(event);
  }

  // ── Camera ──

  getCamera(): Camera {
    const [lon, lat, zoom, pitch, bearing] = this.engine.camera();
    return { center: [lon, lat], zoom, pitch, bearing };
  }

  setCamera(camera: Partial<Camera>): void {
    const c = { ...this.getCamera(), ...camera };
    this.engine.setCamera(c.center[0], c.center[1], c.zoom, c.pitch, c.bearing);
    this.requestRender();
  }

  /** Animated move to a camera, ease-out, over `duration` milliseconds */
  easeTo(camera: Partial<Camera>, duration = 600): void {
    const from = this.getCamera();
    const to = { ...from, ...camera };
    // Shortest way round
    const turn = ((to.bearing - from.bearing + 540) % 360) - 180;
    const start = performance.now();
    this.animate((now) => {
      if (now - start >= duration) this.engine.clearPreload();
      const t = Math.min(1, (now - start) / duration);
      const k = 1 - (1 - t) ** 3;
      const mix = (a: number, b: number) => a + (b - a) * k;
      this.engine.setCamera(
        mix(from.center[0], to.center[0]),
        mix(from.center[1], to.center[1]),
        mix(from.zoom, to.zoom),
        mix(from.pitch, to.pitch),
        from.bearing + turn * k,
      );
      return t < 1;
    });
    // The destination's tiles load during the flight (as Cesium does)
    this.engine.preload(to.center[0], to.center[1], to.zoom, to.pitch, to.bearing);
  }

  setProjection(projection: Projection): void {
    this.engine.setGlobe(projection === 'globe');
    this.requestRender();
  }

  /** Tile counters of the raster layers, for diagnostics */
  stats(): LayerStats[] {
    const s = this.engine.stats();
    const out: LayerStats[] = [];
    for (let i = 0; i < s.length; i += 8) {
      out.push({
        id: s[i],
        drawn: s[i + 1],
        loaded: s[i + 2],
        inFlight: s[i + 3],
        queued: s[i + 4],
        standIns: s[i + 5],
        cooling: s[i + 6],
        missing: s[i + 7],
      });
    }
    return out;
  }

  /**
   * Feature under a screen point (CSS pixels): the network layer and the index of its edge, undefined if none.
   * Reads one pixel of an offscreen picking pass: cheap whatever the number of features, call it on hover.
   */
  pick(point: Point): { layer: number; index: number } | undefined {
    const hit = this.engine.pick(point[0], point[1]);
    return hit && { layer: hit[0], index: hit[1] };
  }

  /** Screen point (CSS pixels) of a position, undefined behind the camera */
  project(lonLat: LonLat): Point | undefined {
    const p = this.engine.project(lonLat[0], lonLat[1]);
    return p && [p[0], p[1]];
  }

  /** Position under a screen point (CSS pixels), undefined above the horizon */
  unproject(point: Point): LonLat | undefined {
    const p = this.engine.ground(point[0], point[1]);
    return p && [p[0], p[1]];
  }

  // ── Layers ──

  /** Replaces the Géoplateforme basemap and hillshade, under the other layers */
  setBasemap(basemap: Basemap, hillshade = 0.4): void {
    this.basemap?.remove();
    this.hillshade?.remove();
    this.basemap = basemap === 'none' ? undefined : this.addRaster(BASEMAPS[basemap]);
    this.engine.setGlobeColor(...SEA[basemap]);
    this.hillshade =
      hillshade > 0 ? this.addRaster({ url: HILLSHADE, minZoom: 6, maxZoom: 17, opacity: hillshade }) : undefined;
    if (this.hillshade) this.engine.moveLayer(this.hillshade.id, 0);
    if (this.basemap) this.engine.moveLayer(this.basemap.id, 0);
  }

  addRaster(options: RasterOptions): RasterLayer {
    const id = this.engine.addRaster(
      options.url,
      options.minZoom ?? 0,
      options.maxZoom ?? 19,
      options.opacity ?? 1,
      options.saturation ?? 1,
    );
    this.requestRender();
    return new RasterLayer(this, id);
  }

  addNetwork(options: NetworkOptions): NetworkLayer {
    const s = { width: 2, max: 1800, contour: 0, opacity: 1, ...options };
    const id = this.engine.addNetwork(options.nodes, options.edges, s.width, s.max, s.contour, s.opacity);
    this.requestRender();
    return new NetworkLayer(this, id, s);
  }

  /** Textured meshes in 3D Tiles (photogrammetry, buildings), drawn with depth over the basemap */
  addTiles3d(options: Tiles3dOptions): Tiles3dLayer {
    const error = options.maxScreenSpaceError ?? 16;
    const id = this.engine.addTiles3d(options.url, error);
    const layer = new Tiles3dLayer(this, id, error);
    if (options.opacity !== undefined) layer.setStyle({ opacity: options.opacity });
    this.requestRender();
    return layer;
  }

  // ── Frames ──

  /** Asks for a frame; several calls before the next frame draw it once */
  requestRender(): void {
    // Changes made by listeners during a frame are drawn by that frame
    if (this.inFrame) return;
    this.frameRequest ||= requestAnimationFrame((now) => this.frame(now));
  }

  /** @internal Runs `step` every frame until it returns false; replaces the running animation */
  animate(step: ((now: number) => boolean) | undefined): void {
    // A flight cut short no longer needs its destination
    if (this.animation) this.engine.clearPreload();
    this.animation = step;
    this.requestRender();
  }

  /** @internal */
  hover(point: Point | undefined): void {
    this.hoverPoint = point;
    if (point) this.requestRender();
  }

  private frame(now: number): void {
    this.frameRequest = 0;
    this.inFrame = true;
    if (this.animation && !this.animation(now)) this.animation = undefined;
    const camera = this.getCamera();
    const key = `${camera.center}|${camera.zoom}|${camera.pitch}|${camera.bearing}`;
    if (key !== this.lastCamera) {
      this.lastCamera = key;
      this.emit('move', camera);
    }
    // Values for the pointer position are computed before drawing, in the same frame
    if (this.hoverPoint) {
      const point = this.hoverPoint;
      this.hoverPoint = undefined;
      this.emit('hover', { point, lonLat: this.unproject(point) });
    }
    const t0 = performance.now();
    this.engine.render();
    this.inFrame = false;
    this.emit('render', { ms: performance.now() - t0 });
    if (this.animation) this.requestRender();
  }

  private resize(): void {
    const { clientWidth: w, clientHeight: h } = this.canvas;
    const ratio = window.devicePixelRatio || 1;
    this.canvas.width = Math.round(w * ratio);
    this.canvas.height = Math.round(h * ratio);
    this.engine.resize(w, h, ratio);
    // Draw at once: a resized canvas is cleared, waiting one frame would flash
    cancelAnimationFrame(this.frameRequest);
    this.frame(performance.now());
  }

  destroy(): void {
    cancelAnimationFrame(this.frameRequest);
    this.resizeObserver.disconnect();
    this.interaction?.destroy();
    this.loader?.destroy();
    this.engine.free();
    this.canvas.remove();
    this.listeners.clear();
  }
}

export class RasterLayer {
  constructor(
    private readonly map: GeoMap,
    readonly id: number,
  ) {}

  setStyle(opacity: number, saturation = 1): void {
    this.map.engine.setRasterStyle(this.id, opacity, saturation);
    this.map.requestRender();
  }

  remove(): void {
    this.map.engine.removeLayer(this.id);
    this.map.requestRender();
  }
}

export class NetworkLayer {
  constructor(
    private readonly map: GeoMap,
    readonly id: number,
    private style: Required<NetworkStyle>,
  ) {}

  /** One value per node, in node order; negative = unreached. Costs one texture upload, drawn next frame. */
  setValues(values: Float32Array): void {
    this.map.engine.setNetworkValues(this.id, values);
    this.map.requestRender();
  }

  setStyle(style: NetworkStyle): void {
    this.style = { ...this.style, ...style };
    const s = this.style;
    this.map.engine.setNetworkStyle(this.id, s.width, s.max, s.contour, s.opacity);
    this.map.requestRender();
  }

  remove(): void {
    this.map.engine.removeLayer(this.id);
    this.map.requestRender();
  }
}

export class Tiles3dLayer {
  private opacity = 1;

  constructor(
    private readonly map: GeoMap,
    readonly id: number,
    private maxScreenSpaceError: number,
  ) {}

  setStyle(style: { maxScreenSpaceError?: number; opacity?: number }): void {
    this.maxScreenSpaceError = style.maxScreenSpaceError ?? this.maxScreenSpaceError;
    this.opacity = style.opacity ?? this.opacity;
    this.map.engine.setTiles3dStyle(this.id, this.maxScreenSpaceError, this.opacity);
    this.map.requestRender();
  }

  stats(): Tiles3dStats {
    const [drawn = 0, loaded = 0, inFlight = 0, gpuMb = 0, tiles = 0] = this.map.engine.tiles3dStats(this.id);
    return { drawn, loaded, inFlight, gpuMb, tiles };
  }

  remove(): void {
    this.map.engine.removeLayer(this.id);
    this.map.requestRender();
  }
}
