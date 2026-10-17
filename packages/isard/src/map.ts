/**
 * The map: canvas, camera, layers, events and the frame loop. Frames are drawn on demand only: an idle map costs
 * nothing. Hover events are coalesced to one per frame, so the values computed for a mouse position are drawn in the
 * same frame.
 */
import { BASEMAPS, HILLSHADE, SEA, TERRAIN_URL } from './basemaps.ts';
import { parseCssColor } from './color.ts';
import { AttributionControl, type Control, type ControlPosition } from './controls.ts';
import { DEFAULT_DECODERS } from './decoders.ts';
import { ExtrusionLayer, type ExtrusionOptions, extrusionStyle } from './extrusions.ts';
import {
  CircleLayer,
  type CircleOptions,
  circleStyle,
  type Feature,
  FillLayer,
  type FillOptions,
  fillStyle,
  LineLayer,
  type LineOptions,
  lineStyle,
} from './features.ts';
import { Glyphs } from './glyphs.ts';
import { Interaction } from './interaction.ts';
import { GroundLayer, type GroundOptions, type Layer, NetworkLayer, RasterLayer, Tiles3dLayer } from './layers.ts';
import { TileLoader } from './loader.ts';
import type { Positioned } from './overlay.ts';
import { injectStyle } from './style.ts';
import { SymbolLayer, type SymbolOptions, symbolStyle } from './symbols.ts';
import type {
  AnimationOptions,
  AtmosphereOptions,
  Basemap,
  FeatureHit,
  FlyToOptions,
  GeoMapOptions,
  LoadingMode,
  LonLat,
  MapEvents,
  NetworkOptions,
  Point,
  Projection,
  RasterOptions,
  TerrainOptions,
  Tiles3dOptions,
} from './types.ts';
import {
  BasemapWorkers,
  defaultWorkerCount,
  defaultWorkerUrl,
  VECTOR_BASEMAPS,
  VectorTiles,
  type VectorTilesOptions,
} from './vectortiles.ts';
import type { Camera, Engine, PickHit, PickResult, RasterLayerStats } from './wasm/isard.js';

/** The engine of a destroyed map: every method a no-op returning undefined, but the camera (the last one) and the
 * bounds, which late callers read */
function destroyed(camera: Camera): Engine {
  return new Proxy(
    {},
    {
      get: (_, name) => {
        if (name === 'camera') return () => camera;
        if (name === 'bounds') return () => Float64Array.of(-180, -85, 180, 85);
        return () => undefined;
      },
    },
  ) as Engine;
}

/** Reports an error without throwing it here: the console shows it, as an uncaught one */
function report(error: unknown): void {
  if (typeof reportError === 'function') reportError(error);
  else
    queueMicrotask(() => {
      throw error;
    });
}

type Listeners = { [K in keyof MapEvents]: Set<(event: MapEvents[K]) => void> };

type EngineClass = new (canvas: HTMLCanvasElement, redraw: () => void) => Engine;

/** An engine loaded: its compiled module and its class */
interface Bindings {
  module: WebAssembly.Module;
  Engine: EngineClass;
}

/** The two builds of the engine, each compiled once per page: `core` without 3D Tiles (a third smaller), loaded by
 * every map; `full` with them, loaded for the first 3D Tiles layer, its module shared with the decoding workers. The
 * core engine has the same API but the 3D Tiles methods: its class is typed as the full one. */
const engines: { core?: Promise<Bindings>; full?: Promise<Bindings> } = {};

function loadEngine(kind: 'core' | 'full', url: string | URL): Promise<Bindings> {
  const loaded = engines[kind];
  if (loaded) return loaded;
  const loading = (async () => {
    // The bindings by dynamic import: an application bundles the ones it loads only
    const [module, bindings] = await Promise.all([
      compile(url),
      kind === 'core' ? import('./wasm/isard_core.js') : import('./wasm/isard.js'),
    ]);
    await bindings.default({ module_or_path: module });
    return { module, Engine: bindings.Engine as unknown as EngineClass };
  })().catch((error: unknown) => {
    // A later call tries again (a network error, a wrong URL fixed)
    delete engines[kind];
    throw error;
  });
  engines[kind] = loading;
  return loading;
}

async function compile(url: string | URL): Promise<WebAssembly.Module> {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`isard: failed to load the engine from ${url}: ${response.status}`);
  try {
    return await WebAssembly.compileStreaming(response.clone());
  } catch {
    // Servers that do not send application/wasm
    return WebAssembly.compile(await response.arrayBuffer());
  }
}

/** Checks a network's arrays before the engine reads them: node index pairs, indices of existing nodes */
function validateNetwork(nodes: Float64Array, edges: Uint32Array): void {
  if (nodes.length % 2 !== 0) throw new Error(`isard: network nodes must be lon, lat pairs (length ${nodes.length})`);
  if (edges.length % 2 !== 0) throw new Error(`isard: network edges must be node index pairs (length ${edges.length})`);
  const count = nodes.length / 2;
  for (let i = 0; i < edges.length; i += 1) {
    const node = edges[i] as number;
    if (node >= count) throw new Error(`isard: network edge ${i >> 1} refers to node ${node}, of ${count} nodes`);
  }
}

/** Relief settings from the option: undefined when off */
function terrainSettings(terrain: boolean | TerrainOptions | undefined): Settings['terrain'] {
  if (!terrain) return undefined;
  const options = terrain === true ? {} : terrain;
  return {
    url: options.url ?? TERRAIN_URL,
    exaggeration: options.exaggeration ?? 1,
    maxZoom: options.maxZoom ?? 15,
    geoid: options.geoid ?? true,
  };
}

/** Sky settings for the engine: colors (RGB 0-1) and blends, MapLibre's defaults */
function atmosphereSettings(options: AtmosphereOptions | undefined): Float32Array | undefined {
  if (!options) return undefined;
  return Float32Array.from([
    ...rgb(options.skyColor ?? '#9cc7ec'),
    ...rgb(options.horizonColor ?? '#e8f1f8'),
    ...rgb(options.fogColor ?? '#f2f5f8'),
    options.skyHorizonBlend ?? 0.6,
    options.horizonFogBlend ?? 0.7,
    options.fogGroundBlend ?? 0.4,
  ]);
}

/** MapLibre's default easing: cubic Bézier (0.25, 0.1, 0.25, 1) */
export function defaultEasing(t: number): number {
  if (t <= 0 || t >= 1) return Math.max(0, Math.min(1, t));
  // Solve x(u) = t for the curve parameter, Newton then bisection
  const [x1, y1, x2, y2] = [0.25, 0.1, 0.25, 1];
  const bezier = (u: number, a: number, b: number) => 3 * a * u * (1 - u) ** 2 + 3 * b * u * u * (1 - u) + u ** 3;
  let u = t;
  for (let i = 0; i < 8; i += 1) {
    const x = bezier(u, x1, x2) - t;
    const dx = 3 * x1 * (1 - u) ** 2 + 6 * (x2 - x1) * u * (1 - u) + 3 * (1 - x2) * u * u;
    if (Math.abs(x) < 1e-6 || dx === 0) break;
    u = Math.max(0, Math.min(1, u - x / dx));
  }
  return bezier(u, y1, y2);
}

const MERCATOR_LAT = 85.051128779806589;
const toMercator = ([lon, lat]: LonLat): [number, number] => {
  const phi = (Math.max(-MERCATOR_LAT, Math.min(MERCATOR_LAT, lat)) * Math.PI) / 180;
  return [(lon + 180) / 360, 0.5 - Math.log(Math.tan(Math.PI / 4 + phi / 2)) / (2 * Math.PI)];
};
const fromMercator = ([x, y]: [number, number]): LonLat => [
  x * 360 - 180,
  (Math.atan(Math.sinh(Math.PI * (1 - 2 * y))) * 180) / Math.PI,
];

/** A CSS color to RGB 0-1 (its alpha dropped); black if unknown */
function rgb(color: string): [r: number, g: number, b: number] {
  const [r, g, b] = parseCssColor(color) ?? [0, 0, 0, 1];
  return [r, g, b];
}

/** Settings replayed on a new engine after a WebGL context loss */
interface Settings {
  zoomRange: [min: number, max: number];
  maxPitch: number;
  globe: boolean;
  sky: boolean;
  atmosphere: Float32Array | undefined;
  terrain: { url: string; exaggeration: number; maxZoom: number; geoid: boolean } | undefined;
  loading: LoadingMode;
  background: [r: number, g: number, b: number];
  globeColor: [r: number, g: number, b: number];
}

const CORNERS: readonly ControlPosition[] = ['top-left', 'top-right', 'bottom-left', 'bottom-right'];

/** A map (see `GeoMap.create`); layers take it as their host */
export class GeoMap {
  readonly canvas: HTMLCanvasElement;
  #glyphs: Glyphs | undefined;
  /** Layers new layers go under */
  private readonly onTop = new Set<Layer>();
  /** The vector basemap, if any: its tiles' workers and its labels */
  private vector: { tiles: VectorTiles; labels: SymbolLayer<unknown> } | undefined;
  private readonly glyphsUrl: string | undefined;
  private readonly basemapWorkerUrl: string | URL | undefined;
  private readonly container: HTMLElement;
  /** Markers and popups, over the canvas */
  private readonly overlay: HTMLDivElement;
  private readonly corners: Record<ControlPosition, HTMLDivElement>;
  private readonly controls = new Map<Control, HTMLElement>();
  private readonly positioned = new Set<Positioned>();
  /** Canvas size in CSS pixels, kept on resize so placing overlays never reads the layout */
  private size: Point = [0, 0];
  #engine: Engine;
  /** Layers in drawing order (bottom first), to recreate them after a context loss */
  private readonly layers: Layer[] = [];
  private readonly settings: Settings;
  /** The WebGL context is lost: nothing is drawn until it is restored */
  private lost = false;
  private readonly listeners: Listeners = {
    movestart: new Set(),
    move: new Set(),
    moveend: new Set(),
    resize: new Set(),
    hover: new Set(),
    hoverfeature: new Set(),
    click: new Set(),
    render: new Set(),
    idle: new Set(),
    leave: new Set(),
  };
  private readonly resizeObserver: ResizeObserver;
  private readonly interaction: Interaction | undefined;
  private loader: TileLoader | undefined;
  /** The class of the engine: without 3D Tiles until a 3D Tiles layer is added, unless asked for from the start */
  private engineClass: EngineClass;
  /** The module of the engine with 3D Tiles once it runs this map (the decoding workers run it too) */
  private tiles3dModule: WebAssembly.Module | undefined;
  /** The engine with 3D Tiles is loading, for the 3D Tiles layers added on the core engine */
  private upgrading: Promise<void> | undefined;
  /** What loading the engine with 3D Tiles and its workers needs */
  private readonly tiles3dOptions: Pick<GeoMapOptions, 'wasmUrl' | 'workers' | 'workerUrl' | 'decoders'>;
  private gone = false;
  private basemap: RasterLayer | undefined;
  private hillshade: RasterLayer | undefined;
  private animation: ((now: number) => boolean) | undefined;
  private hoverPoint: Point | undefined;
  /** Frame polling the engine for the answer of the hover picking, 0 if none */
  private hoverPoll = 0;
  /** What 'hoverfeature' looks for: within `radius` CSS pixels, in `layers` only if given */
  hoverPicking: { radius?: number; layers?: readonly Layer[] } = {};
  private frameRequest = 0;
  private lastCamera = '';
  /** The camera moved since the last 'moveend' */
  private moving = false;
  /** The last frame found everything loaded ('idle' sent) */
  private settled = false;
  private inFrame = false;
  /** Vector tiles made by `vectorTiles()`, destroyed with the map */
  private readonly vectorSets = new Set<VectorTiles>();
  /** The workers drawing vector tiles, shared by every `VectorTiles` of the map; created on first use */
  private basemapPool: BasemapWorkers | undefined;
  /** Stops watching the device pixel ratio */
  private unwatchRatio: (() => void) | undefined;

  /** Loads the engine (once per page) and creates a map filling `container` */
  static async create(container: HTMLElement, options: GeoMapOptions = {}): Promise<GeoMap> {
    // With 3D Tiles from the start when asked, or when only that engine's URL is given (as before the core engine)
    const full = options.tiles3d === true || (options.wasmUrl !== undefined && options.coreWasmUrl === undefined);
    const bindings = full
      ? await loadEngine('full', options.wasmUrl ?? new URL('./isard_bg.wasm', import.meta.url))
      : await loadEngine('core', options.coreWasmUrl ?? new URL('./isard_core_bg.wasm', import.meta.url));
    return new GeoMap(container, options, bindings.Engine, full ? bindings.module : undefined);
  }

  private constructor(
    container: HTMLElement,
    options: GeoMapOptions,
    engineClass: EngineClass,
    tiles3dModule: WebAssembly.Module | undefined,
  ) {
    this.engineClass = engineClass;
    this.tiles3dModule = tiles3dModule;
    this.tiles3dOptions = { ...options };
    this.glyphsUrl = options.glyphs;
    this.basemapWorkerUrl = options.basemapWorkerUrl;
    this.container = container;
    this.canvas = document.createElement('canvas');
    Object.assign(this.canvas.style, { display: 'block', width: '100%', height: '100%', touchAction: 'none' });
    // Overlays and controls are positioned against the container
    injectStyle(container.ownerDocument);
    if (getComputedStyle(container).position === 'static') container.style.position = 'relative';
    container.classList.add('isard-map');
    this.overlay = document.createElement('div');
    this.overlay.className = 'isard-overlay';
    const corner = (position: ControlPosition) => {
      const element = document.createElement('div');
      element.className = `isard-ctrl-${position}`;
      return element;
    };
    this.corners = {
      'top-left': corner('top-left'),
      'top-right': corner('top-right'),
      'bottom-left': corner('bottom-left'),
      'bottom-right': corner('bottom-right'),
    };
    container.append(this.canvas, this.overlay, ...CORNERS.map((position) => this.corners[position]));
    this.settings = {
      zoomRange: [options.minZoom ?? 0, options.maxZoom ?? 22],
      maxPitch: options.maxPitch ?? 70,
      globe: (options.projection ?? 'globe') === 'globe',
      sky: options.sky ?? true,
      atmosphere: atmosphereSettings(options.atmosphere),
      terrain: terrainSettings(options.terrain),
      loading: options.loading ?? 'immediate',
      background: rgb(options.background ?? '#f2f2f0'),
      globeColor: SEA['plan-grey'],
    };
    this.#engine = this.createEngine();
    // The browser may take the GPU away (driver reset, background tab on mobile): the map waits, then rebuilds
    this.canvas.addEventListener('webglcontextlost', (event) => {
      event.preventDefault();
      this.lost = true;
      cancelAnimationFrame(this.frameRequest);
      this.frameRequest = 0;
    });
    this.canvas.addEventListener('webglcontextrestored', () => this.restore());
    this.setCamera({ center: [2.4246, 48.8457], zoom: 13, pitch: 0, bearing: 0, ...options });
    this.setBasemap(options.basemap ?? 'plan-grey', options.hillshade ?? 0.4);
    this.resizeObserver = new ResizeObserver(() => this.resize());
    this.resizeObserver.observe(container);
    this.watchRatio();
    this.resize();
    const interactive = options.interactive ?? true;
    const keyboard = interactive && (options.keyboard ?? true);
    this.interaction = interactive ? new Interaction(this, { keyboard }) : undefined;
    // Reached with Tab when the keys move it; named for screen readers either way
    this.canvas.classList.add('isard-canvas');
    this.canvas.setAttribute('role', keyboard ? 'application' : 'region');
    this.canvas.setAttribute('aria-roledescription', 'map');
    this.canvas.setAttribute('aria-label', options.ariaLabel ?? 'Map');
    if (keyboard) this.canvas.tabIndex = 0;
    const attribution = options.attributionControl ?? true;
    if (attribution !== false) {
      this.addControl(new AttributionControl(attribution === true ? {} : attribution), 'bottom-right');
    }
  }

  /** The element the map was created in */
  getContainer(): HTMLElement {
    return this.container;
  }

  getCanvas(): HTMLCanvasElement {
    return this.canvas;
  }

  /** @internal The engine (replaced after a context loss) */
  get engine(): Engine {
    return this.#engine;
  }

  private createEngine(): Engine {
    const engine = new this.engineClass(this.canvas, () => this.requestRender());
    const { zoomRange, maxPitch, globe, sky, atmosphere, terrain, loading, background, globeColor } = this.settings;
    engine.setZoomRange(...zoomRange);
    engine.setMaxPitch(maxPitch);
    engine.setGlobe(globe);
    engine.setSky(sky);
    engine.setAtmosphere(atmosphere);
    engine.setTerrain(terrain?.url, terrain?.exaggeration ?? 1, terrain?.maxZoom, terrain?.geoid);
    engine.setImmediateLoading(loading === 'immediate');
    engine.setBackground(...background);
    engine.setGlobeColor(...globeColor);
    this.loader ??= this.startLoader(engine);
    if (this.loader) engine.setTileLoader(this.loader);
    return engine;
  }

  /** The decoding workers of 3D Tiles, with the engine that has them (they run its module) and unless turned off */
  private startLoader(engine: Engine): TileLoader | undefined {
    const { workers: count, workerUrl, decoders } = this.tiles3dOptions;
    const workers = count ?? Math.min(4, Math.max(1, (navigator.hardwareConcurrency || 2) - 1));
    if (!this.tiles3dModule || workers === false || workers <= 0 || typeof Worker === 'undefined') return undefined;
    return new TileLoader(
      this.tiles3dModule,
      workerUrl ?? new URL('./decoder.worker.js', import.meta.url),
      workers,
      { ...DEFAULT_DECODERS, ...decoders },
      engine.textureFormats(),
    );
  }

  /** A 3D Tiles layer on the core engine: loads the engine with 3D Tiles, then rebuilds the map on it (camera,
   * settings and layers kept, as after a context loss); the layer is created then */
  private upgrade(): void {
    const url = this.tiles3dOptions.wasmUrl ?? new URL('./isard_bg.wasm', import.meta.url);
    this.upgrading ??= loadEngine('full', url).then(
      ({ module, Engine }) => {
        this.upgrading = undefined;
        if (this.gone) return;
        this.engineClass = Engine;
        this.tiles3dModule = module;
        // A lost context: the engine is rebuilt on it once restored
        if (!this.lost) this.restore();
      },
      (error: unknown) => {
        // The 3D Tiles layers stay empty; the next one added tries again
        this.upgrading = undefined;
        report(error);
        this.requestRender();
      },
    );
  }

  /** After a context loss: a new engine with the same camera, settings and layers (same handles, same order) */
  private restore(): void {
    this.cancelHoverPick();
    const camera = this.getCamera();
    this.#engine.free();
    this.#engine = this.createEngine();
    this.lost = false;
    this.setCamera(camera);
    this.#glyphs?.attach(this.#engine);
    for (const layer of this.layers) layer.attach(this.#engine);
    this.resize();
  }

  /** @internal Forgets a removed layer */
  detach(layer: object): void {
    const index = this.layers.indexOf(layer as Layer);
    if (index >= 0) this.layers.splice(index, 1);
  }

  private add<L extends Layer>(layer: L): L {
    // On the core engine, a 3D Tiles layer waits for the engine with 3D Tiles: it is created with the others on it
    if (layer instanceof Tiles3dLayer && !this.tiles3dModule) this.upgrade();
    else layer.attach(this.engine);
    this.layers.push(layer);
    // Under the layers kept on top (the basemap's labels)
    const top = this.layers.find((l) => this.onTop.has(l) && l !== layer);
    if (top) this.moveLayer(layer, top);
    this.requestRender();
    return layer;
  }

  /** @internal Keeps a layer above the layers added after it (they go under it), as the labels of a basemap */
  keepOnTop(layer: Layer): void {
    this.onTop.add(layer);
  }

  /** @internal Moves a layer under `before`, or on top of all */
  moveLayer(layer: object, before: object | undefined): void {
    const handle = layer as Layer;
    this.detach(handle);
    const index = before ? this.layers.indexOf(before as Layer) : -1;
    const at = index < 0 ? this.layers.length : index;
    this.layers.splice(at, 0, handle);
    if (handle.id >= 0) this.engine.moveLayer(handle.id, this.engineIndex(at));
    this.requestRender();
  }

  /** @internal See `LayerHost.replaceLayer` */
  replaceLayer(layer: object, old: number): void {
    this.engine.removeLayer(old);
    const index = this.layers.indexOf(layer as Layer);
    if (index >= 0) this.engine.moveLayer((layer as Layer).id, this.engineIndex(index));
  }

  /** Index in the engine of the layer at `index`: the layers waiting for the engine with 3D Tiles are not in it yet */
  private engineIndex(index: number): number {
    let count = 0;
    for (let i = 0; i < index; i += 1) if ((this.layers[i]?.id ?? -1) >= 0) count += 1;
    return count;
  }

  /** Moves a layer under all the others */
  private toBottom(layer: Layer): void {
    this.engine.moveLayer(layer.id, 0);
    this.detach(layer);
    this.layers.unshift(layer);
  }

  // ── Events ──

  /** Listens to an event; returns the function that stops listening */
  on<K extends keyof MapEvents>(type: K, listener: (event: MapEvents[K]) => void): () => void {
    const listeners: Set<(event: MapEvents[K]) => void> = this.listeners[type];
    listeners.add(listener);
    return () => listeners.delete(listener);
  }

  /** @internal Calls the listeners; one that throws is reported and does not stop the others nor the frame */
  emit<K extends keyof MapEvents>(type: K, event: MapEvents[K]): void {
    const listeners: Set<(event: MapEvents[K]) => void> = this.listeners[type];
    for (const listener of listeners) {
      try {
        listener(event);
      } catch (error) {
        report(error);
      }
    }
  }

  // ── Camera ──

  getCamera(): Camera {
    return this.engine.camera();
  }

  setCamera(camera: Partial<Camera>): void {
    const c = { ...this.getCamera(), ...camera };
    this.engine.setCamera(c.center[0], c.center[1], c.zoom, c.pitch, c.bearing);
    this.requestRender();
  }

  /** Animated move to a camera: over `duration` milliseconds (or the options), ease-out by default */
  easeTo(camera: Partial<Camera>, options: number | AnimationOptions = 600): void {
    const { duration = 600, easing = (t: number) => 1 - (1 - t) ** 3 } =
      typeof options === 'number' ? { duration: options } : options;
    const from = this.getCamera();
    const to = { ...from, ...camera };
    // Shortest way round
    const turn = ((to.bearing - from.bearing + 540) % 360) - 180;
    const start = performance.now();
    this.animate((now) => {
      if (now - start >= duration) this.engine.clearPreload();
      const t = duration > 0 ? Math.min(1, (now - start) / duration) : 1;
      const k = easing(t);
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

  /**
   * Flight to a camera that zooms out on its way and back in (van Wijk and Nuij's optimal path, as MapLibre's
   * `flyTo`): long moves stay readable. Pitch and bearing follow the progress.
   */
  flyTo(camera: Partial<Camera>, options: FlyToOptions = {}): void {
    const from = this.getCamera();
    const to = { ...from, ...camera };
    const { curve: rho = 1.42, speed = 1.2, easing = defaultEasing } = options;
    const [x0, y0] = toMercator(from.center);
    const [x1, y1] = toMercator(to.center);
    // Across the antimeridian, the short way
    const dx = x1 - x0 - Math.round(x1 - x0);
    const dy = y1 - y0;
    // In pixels at the start zoom (512 px world at zoom 0)
    const world = 512 * 2 ** from.zoom;
    const u1 = Math.hypot(dx, dy) * world;
    const w0 = Math.max(this.canvas.clientWidth, this.canvas.clientHeight, 1);
    const w1 = w0 / 2 ** (to.zoom - from.zoom);
    const rho2 = rho * rho;
    const r = (i: 0 | 1) => {
      const b = (w1 * w1 - w0 * w0 + (i ? -1 : 1) * rho2 * rho2 * u1 * u1) / (2 * (i ? w1 : w0) * rho2 * u1);
      return Math.log(Math.sqrt(b * b + 1) - b);
    };
    // No move on the ground (or nearly): a plain zoom
    const still = u1 < 1e-6 || !Number.isFinite(r(0)) || !Number.isFinite(r(1));
    const r0 = still ? 0 : r(0);
    const length = still ? Math.abs(Math.log(w1 / w0)) / rho : (r(1) - r0) / rho;
    const width = (s: number) =>
      still ? Math.exp((w1 < w0 ? -1 : 1) * rho * s) : Math.cosh(r0) / Math.cosh(r0 + rho * s);
    const along = (s: number) =>
      still ? 0 : (w0 * ((Math.cosh(r0) * Math.tanh(r0 + rho * s) - Math.sinh(r0)) / rho2)) / u1;
    const duration = options.duration ?? (1000 * length) / speed;
    const turn = ((to.bearing - from.bearing + 540) % 360) - 180;
    const start = performance.now();
    this.animate((now) => {
      if (now - start >= duration) this.engine.clearPreload();
      const t = duration > 0 ? Math.min(1, (now - start) / duration) : 1;
      const k = easing(t);
      const s = k * length;
      const u = t >= 1 ? 1 : along(s);
      const zoom = t >= 1 ? to.zoom : from.zoom + Math.log2(1 / width(s));
      const [lon, lat] = fromMercator([x0 + dx * u, y0 + dy * u]);
      this.engine.setCamera(lon, lat, zoom, from.pitch + (to.pitch - from.pitch) * k, from.bearing + turn * k);
      return t < 1;
    });
    this.engine.preload(to.center[0], to.center[1], to.zoom, to.pitch, to.bearing);
  }

  /** Longitudes and latitudes the view covers on the ground: `[west, south, east, north]` */
  getBounds(): [west: number, south: number, east: number, north: number] {
    const [west = -180, south = -85, east = 180, north = 85] = this.engine.bounds();
    return [west, south, east, north];
  }

  /** Lowest zoom the camera accepts (the widest view) */
  setMinZoom(zoom: number): void {
    this.settings.zoomRange[0] = zoom;
    this.engine.setZoomRange(...this.settings.zoomRange);
    this.requestRender();
  }

  setMaxZoom(zoom: number): void {
    this.settings.zoomRange[1] = zoom;
    this.engine.setZoomRange(...this.settings.zoomRange);
    this.requestRender();
  }

  /** Highest pitch, degrees, up to 85 */
  setMaxPitch(pitch: number): void {
    this.settings.maxPitch = pitch;
    this.engine.setMaxPitch(pitch);
    this.requestRender();
  }

  /** Sky over the plane and the fog of the ground (see `GeoMapOptions.atmosphere`); undefined to remove them */
  setAtmosphere(options: AtmosphereOptions | undefined): void {
    this.settings.atmosphere = atmosphereSettings(options);
    this.engine.setAtmosphere(this.settings.atmosphere);
    this.requestRender();
  }

  setProjection(projection: Projection): void {
    this.settings.globe = projection === 'globe';
    this.engine.setGlobe(this.settings.globe);
    this.requestRender();
  }

  /** How tiles load and show (see `LoadingMode`) */
  setLoading(mode: LoadingMode): void {
    this.settings.loading = mode;
    this.engine.setImmediateLoading(mode === 'immediate');
    this.requestRender();
  }

  /** Space behind the globe (stars, atmosphere halo) */
  setSky(enabled: boolean): void {
    this.settings.sky = enabled;
    this.engine.setSky(enabled);
    this.requestRender();
  }

  /** Relief under the basemap: true for the IGN terrain model, its options, or false */
  setTerrain(terrain: boolean | TerrainOptions): void {
    this.settings.terrain = terrainSettings(terrain);
    const t = this.settings.terrain;
    this.engine.setTerrain(t?.url, t?.exaggeration ?? 1, t?.maxZoom, t?.geoid);
    this.requestRender();
  }

  /** Color behind the map, a CSS color (its alpha ignored) */
  setBackground(color: string): void {
    this.settings.background = rgb(color);
    this.engine.setBackground(...this.settings.background);
    this.requestRender();
  }

  /** Tile counters of the raster layers, for diagnostics */
  stats(): RasterLayerStats[] {
    return this.engine.stats();
  }

  /**
   * Network edge under a screen point (CSS pixels), undefined if none. Reads one pixel of an offscreen picking pass:
   * cheap whatever the number of features, call it on hover.
   */
  pick(point: Point, radius = 0, layers?: readonly Layer[]): PickHit | undefined {
    const only = layers && Uint32Array.from(layers.map((l) => l.id));
    return this.engine.pick(point[0], point[1], radius, only);
  }

  /**
   * The feature under a screen point (CSS pixels), or the nearest one within `radius` pixels: its layer handle, its
   * index there and, for line, circle and fill layers, the feature itself; undefined if none. `layers` restricts the
   * search to some layers (the others hide nothing).
   */
  pickFeature(point: Point, options: { radius?: number; layers?: readonly Layer[] } = {}): FeatureHit | undefined {
    const hit = this.pick(point, options.radius ?? 0, options.layers);
    return hit && this.featureHit(hit);
  }

  /** A hit with its layer handle and feature; undefined if the layer is gone */
  private featureHit(hit: PickHit): FeatureHit | undefined {
    const layer = this.layers.find((l) => l.id === hit.layer);
    if (!layer) return undefined;
    const feature =
      'featureAt' in layer && typeof layer.featureAt === 'function'
        ? (layer.featureAt(hit.index) as Feature | undefined)
        : undefined;
    return { layer, index: hit.index, feature };
  }

  /** Asks the engine what is under the pointer; its answer, read without waiting for the GPU, comes on a later frame */
  private requestHoverPick(point: Point): void {
    const { radius = 0, layers } = this.hoverPicking;
    const only = layers && Uint32Array.from(layers.map((l) => l.id));
    // A new request replaces the one in flight (in the engine too): one poll loop at most
    if (!this.engine.requestPick(point[0], point[1], radius, only) || this.hoverPoll) return;
    this.hoverPoll = requestAnimationFrame(this.pollHoverPick);
  }

  private readonly pollHoverPick = (): void => {
    this.hoverPoll = 0;
    if (this.lost) return;
    const result: PickResult | undefined = this.engine.pollPick();
    if (!result) {
      // The GPU is not done yet (usually a frame): asked again next frame, without drawing
      this.hoverPoll = requestAnimationFrame(this.pollHoverPick);
      return;
    }
    this.emit('hoverfeature', {
      point: [result.x, result.y],
      lonLat: result.lonLat,
      hit: result.hit && this.featureHit(result.hit),
    });
  };

  private cancelHoverPick(): void {
    cancelAnimationFrame(this.hoverPoll);
    this.hoverPoll = 0;
  }

  /** Screen point (CSS pixels) of a position, undefined behind the camera */
  project(lonLat: LonLat): Point | undefined {
    return this.engine.project(lonLat[0], lonLat[1]);
  }

  /** Position under a screen point (CSS pixels), undefined above the horizon */
  unproject(point: Point): LonLat | undefined {
    return this.engine.ground(point[0], point[1]);
  }

  // ── Overlays and controls ──

  /** Adds a control in a corner of the map; controls stack from the corner inward */
  addControl(control: Control, position: ControlPosition = 'top-right'): this {
    if (this.controls.has(control)) return this;
    const element = control.onAdd(this);
    element.classList.add('isard-ctrl');
    // In the bottom corners the first control added stays lowest, as in MapLibre
    if (position.startsWith('bottom')) this.corners[position].prepend(element);
    else this.corners[position].append(element);
    this.controls.set(control, element);
    return this;
  }

  removeControl(control: Control): this {
    const element = this.controls.get(control);
    if (!element) return this;
    this.controls.delete(control);
    control.onRemove(this);
    element.remove();
    return this;
  }

  /** @internal Shows a marker or popup, placed after every frame from now on */
  attachOverlay(item: Positioned, element: HTMLElement): void {
    this.overlay.append(element);
    this.positioned.add(item);
    item.position(this.size);
  }

  /** @internal */
  detachOverlay(item: Positioned, element: HTMLElement): void {
    this.positioned.delete(item);
    element.remove();
  }

  /** @internal Places one overlay now (it moved), without drawing a frame */
  positionOverlay(item: Positioned): void {
    if (this.positioned.has(item)) item.position(this.size);
  }

  // ── Layers ──

  /** Replaces the Géoplateforme basemap and hillshade, under the other layers */
  setBasemap(basemap: Basemap, hillshade = 0.4): void {
    this.basemap?.remove();
    this.hillshade?.remove();
    if (this.vector) {
      this.onTop.delete(this.vector.labels as Layer);
      this.vector.labels.remove();
      this.vector.tiles.destroy();
      this.vector = undefined;
    }
    const vector = VECTOR_BASEMAPS[basemap as keyof typeof VECTOR_BASEMAPS];
    if (vector) {
      const tiles = new VectorTiles(this, {
        style: vector,
        ...(this.basemapWorkerUrl ? { workerUrl: this.basemapWorkerUrl } : {}),
      });
      this.basemap = tiles.raster();
      this.vector = { tiles, labels: tiles.labels() as SymbolLayer<unknown> };
    } else {
      this.basemap = basemap === 'none' ? undefined : this.addRaster(BASEMAPS[basemap as keyof typeof BASEMAPS]);
    }
    this.settings.globeColor = SEA[basemap];
    this.engine.setGlobeColor(...this.settings.globeColor);
    this.hillshade = hillshade > 0 ? this.addRaster({ ...HILLSHADE, opacity: hillshade }) : undefined;
    if (this.hillshade) this.toBottom(this.hillshade);
    if (this.basemap) this.toBottom(this.basemap);
  }

  /** Raster tiles in Web Mercator ({z}, {x}, {y} URL), over the layers already added */
  addRaster(options: RasterOptions & { before?: Layer }): RasterLayer {
    const { url, minZoom = 0, maxZoom = 19, opacity = 1, saturation = 1, size, loader, before } = options;
    const layer = new RasterLayer(this, { url, minZoom, maxZoom, opacity, saturation, size, loader });
    return this.place(layer, before);
  }

  /** Line network colored by a value per node; `layer.setValues` recolors it in one texture upload */
  addNetwork(options: NetworkOptions): NetworkLayer {
    const { nodes, edges, lines, costs, edgeWidths, width = 2, max = 1800, contour = 0, opacity = 1 } = options;
    const { halo = 0, haloOpacity = 0.11, ramp, unreachedColor = 'rgba(140, 140, 140, 0.25)', fade = 0 } = options;
    const style = { width, max, contour, opacity, halo, haloOpacity, ramp, unreachedColor, fade };
    validateNetwork(nodes, edges);
    return this.add(new NetworkLayer(this, { nodes, edges, lines }, costs, style, edgeWidths));
  }

  /** Lines from GeoJSON (line strings, and polygon outlines), styled per feature */
  addLines<P = Record<string, unknown>>(options: LineOptions<P>): LineLayer<P> {
    return this.place(new LineLayer<P>(this, options.data, lineStyle(options)), options.before);
  }

  /** Discs from GeoJSON points, styled per feature */
  addCircles<P = Record<string, unknown>>(options: CircleOptions<P>): CircleLayer<P> {
    return this.place(new CircleLayer<P>(this, options.data, circleStyle(options)), options.before);
  }

  /** Filled polygons from GeoJSON (holes included), styled per feature; over the relief they lie on the ground */
  addFills<P = Record<string, unknown>>(options: FillOptions<P>): FillLayer<P> {
    return this.place(new FillLayer<P>(this, options.data, fillStyle(options)), options.before);
  }

  /** The application's fragment shader over the ground (see `GroundOptions`); under `before` if given */
  addGround(options: GroundOptions & { before?: Layer }): GroundLayer {
    return this.place(new GroundLayer(this, options), options.before);
  }

  /** Polygons raised into blocks with depth (buildings), styled per feature; tiles of them come with `setTile` */
  addExtrusions<P = Record<string, unknown>>(options: ExtrusionOptions<P> = {}): ExtrusionLayer<P> {
    return this.place(new ExtrusionLayer<P>(this, options.data, extrusionStyle(options)), options.before);
  }

  /** Text labels from GeoJSON (points, the middle of lines, the center of polygons), placed without overlaps */
  addSymbols<P = Record<string, unknown>>(options: SymbolOptions<P>): SymbolLayer<P> {
    return this.place(new SymbolLayer<P>(this, options.data, symbolStyle(options)), options.before);
  }

  /** @internal The glyphs of the symbol layers, loaded on first use */
  get glyphs(): Glyphs {
    if (!this.#glyphs) {
      // The engine's context: the atlas stays within its texture size limit
      const gl = this.canvas.getContext('webgl2');
      const max = gl?.getParameter(gl.MAX_TEXTURE_SIZE);
      this.#glyphs = new Glyphs(this.glyphsUrl, typeof max === 'number' ? max : undefined);
      this.#glyphs.attach(this.engine);
    }
    return this.#glyphs;
  }

  /**
   * Vector tiles with a MapLibre style (PLAN.IGN grey by default): `tiles.raster(…)` draws its fills and lines (or a
   * subset) as a raster layer, `tiles.labels(…)` its labels. `setBasemap('plan-grey-vector')` does both for the basemap.
   */
  vectorTiles(options: VectorTilesOptions = {}): VectorTiles {
    const tiles = new VectorTiles(this, {
      ...(this.basemapWorkerUrl ? { workerUrl: this.basemapWorkerUrl } : {}),
      ...options,
    });
    // Its workers stop with the map
    this.vectorSets.add(tiles);
    return tiles;
  }

  /**
   * @internal The workers for a `VectorTiles`: the map's shared pool, or a pool of its own (`own`, to stop with it)
   * when it asks for another count or URL
   */
  basemapWorkers(count?: number, url?: string | URL): { pool: BasemapWorkers; own: boolean } {
    const sharedUrl = this.basemapWorkerUrl ?? defaultWorkerUrl();
    const shared = defaultWorkerCount();
    const workers = Math.max(1, count ?? shared);
    const workerUrl = url ?? sharedUrl;
    if (workers === shared && String(workerUrl) === String(sharedUrl)) {
      this.basemapPool ??= new BasemapWorkers(shared, sharedUrl);
      return { pool: this.basemapPool, own: false };
    }
    return { pool: new BasemapWorkers(workers, workerUrl), own: true };
  }

  /** Adds a layer, under `before` if given */
  private place<L extends Layer>(layer: L, before: Layer | undefined): L {
    this.add(layer);
    if (before) this.moveLayer(layer, before);
    return layer;
  }

  /** Textured meshes in 3D Tiles (photogrammetry, buildings), drawn with depth over the basemap */
  addTiles3d(options: Tiles3dOptions): Tiles3dLayer {
    const { url, maxScreenSpaceError = 16, opacity = 1 } = options;
    return this.add(new Tiles3dLayer(this, url, { maxScreenSpaceError, opacity }));
  }

  // ── Frames ──

  /** Asks for a frame; several calls before the next frame draw it once */
  requestRender(): void {
    // Changes made by listeners during a frame are drawn by that frame; nothing is drawn without a context
    if (this.inFrame || this.lost) return;
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
    else this.cancelHoverPick();
  }

  private frame(now: number): void {
    this.frameRequest = 0;
    try {
      this.inFrame = true;
      this.drawFrame(now);
    } finally {
      // Whatever throws in a frame, the next ones are still drawn
      this.inFrame = false;
    }
  }

  private drawFrame(now: number): void {
    if (this.animation && !this.animation(now)) this.animation = undefined;
    const camera = this.getCamera();
    const key = `${camera.center}|${camera.zoom}|${camera.pitch}|${camera.bearing}`;
    const moved = key !== this.lastCamera;
    if (moved) {
      // The first frame is no move
      if (!this.moving && this.lastCamera !== '') {
        this.moving = true;
        this.emit('movestart', camera);
      }
      this.lastCamera = key;
      this.emit('move', camera);
    } else if (this.moving && !this.animation && !this.interaction?.active) {
      this.moving = false;
      this.emit('moveend', camera);
    }
    // Values for the pointer position are computed before drawing, in the same frame
    let hovered: Point | undefined;
    if (this.hoverPoint) {
      const point = this.hoverPoint;
      hovered = point;
      this.hoverPoint = undefined;
      this.emit('hover', { point, lonLat: this.unproject(point) });
    }
    const t0 = performance.now();
    // The engine asks for the next frame while it draws (uploads past the frame budget, fades): not ignored
    this.inFrame = false;
    const loading = this.engine.render();
    this.inFrame = true;
    // The picking pass sees the frame just drawn; its answer comes on a later frame
    if (hovered && this.listeners.hoverfeature.size > 0) this.requestHoverPick(hovered);
    // Overlays follow the frame just drawn (camera, relief altitudes): all in one pass, transforms only
    for (const item of this.positioned) item.position(this.size);
    this.emit('render', { ms: performance.now() - t0 });
    this.inFrame = false;
    // A move goes on until a frame finds the camera still: that frame sends 'moveend'
    if (this.animation || (moved && this.moving)) this.requestRender();
    // Idle: nothing loading (the engine with 3D Tiles neither), no animation, no frame asked for (uploads waiting,
    // fades, listeners)
    if (loading || this.upgrading || this.animation || this.frameRequest !== 0) {
      this.settled = false;
    } else if (!this.settled) {
      this.settled = true;
      this.emit('idle', {});
    }
  }

  /** Resizes when the device pixel ratio changes (browser zoom, a window moved to another screen) */
  private watchRatio(): void {
    if (typeof matchMedia !== 'function') return;
    const query = matchMedia(`(resolution: ${window.devicePixelRatio || 1}dppx)`);
    const changed = () => {
      // A query for the new ratio
      this.unwatchRatio?.();
      this.watchRatio();
      this.resize();
    };
    query.addEventListener('change', changed, { once: true });
    this.unwatchRatio = () => query.removeEventListener('change', changed);
  }

  private resize(): void {
    if (this.lost) return;
    const { clientWidth: width, clientHeight: height } = this.canvas;
    const ratio = window.devicePixelRatio || 1;
    this.canvas.width = Math.round(width * ratio);
    this.canvas.height = Math.round(height * ratio);
    this.engine.resize(width, height, ratio);
    this.size = [width, height];
    this.emit('resize', { width, height });
    // Draw at once: a resized canvas is cleared, waiting one frame would flash
    cancelAnimationFrame(this.frameRequest);
    this.frame(performance.now());
  }

  destroy(): void {
    this.gone = true;
    const camera = this.getCamera();
    cancelAnimationFrame(this.frameRequest);
    this.cancelHoverPick();
    this.vector?.tiles.destroy();
    for (const tiles of this.vectorSets) tiles.destroy();
    this.vectorSets.clear();
    this.basemapPool?.terminate();
    this.basemapPool = undefined;
    this.unwatchRatio?.();
    this.unwatchRatio = undefined;
    this.resizeObserver.disconnect();
    this.interaction?.destroy();
    this.loader?.destroy();
    this.engine.free();
    // Work still under way (labels being shaped, tiles arriving, timers) may call the engine after this: it finds one
    // that does nothing, and nothing is drawn again
    this.#engine = destroyed(camera);
    this.lost = true;
    for (const control of [...this.controls.keys()]) this.removeControl(control);
    this.positioned.clear();
    this.canvas.remove();
    this.overlay.remove();
    for (const position of CORNERS) this.corners[position].remove();
    this.container.classList.remove('isard-map');
    for (const listeners of Object.values(this.listeners)) listeners.clear();
  }
}
