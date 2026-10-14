/**
 * The map: canvas, camera, layers, events and the frame loop. Frames are drawn on demand only: an idle map costs
 * nothing. Hover events are coalesced to one per frame, so the values computed for a mouse position are drawn in the
 * same frame.
 */
import { BASEMAPS, HILLSHADE, SEA } from './basemaps.ts';
import { DEFAULT_DECODERS } from './decoders.ts';
import { Interaction } from './interaction.ts';
import { type Layer, NetworkLayer, RasterLayer, Tiles3dLayer } from './layers.ts';
import { TileLoader } from './loader.ts';
import type {
  Basemap,
  GeoMapOptions,
  LayerHost,
  LonLat,
  MapEvents,
  NetworkOptions,
  Point,
  Projection,
  RasterOptions,
  Tiles3dOptions,
} from './types.ts';
import type { Camera, PickHit, RasterLayerStats } from './wasm/geo_render.js';
import init, { Engine } from './wasm/geo_render.js';

type Listeners = { [K in keyof MapEvents]: Set<(event: MapEvents[K]) => void> };

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

/** `#rrggbb` to RGB 0-1 */
function rgb(hex: string): [r: number, g: number, b: number] {
  const value = Number.parseInt(hex.replace('#', ''), 16);
  return [((value >> 16) & 255) / 255, ((value >> 8) & 255) / 255, (value & 255) / 255];
}

/** Settings replayed on a new engine after a WebGL context loss */
interface Settings {
  zoomRange: [min: number, max: number];
  globe: boolean;
  sky: boolean;
  background: [r: number, g: number, b: number];
  globeColor: [r: number, g: number, b: number];
}

export class GeoMap implements LayerHost {
  readonly canvas: HTMLCanvasElement;
  #engine: Engine;
  /** Layers in drawing order (bottom first), to recreate them after a context loss */
  private readonly layers: Layer[] = [];
  private readonly settings: Settings;
  /** The WebGL context is lost: nothing is drawn until it is restored */
  private lost = false;
  private readonly listeners: Listeners = { move: new Set(), hover: new Set(), click: new Set(), render: new Set() };
  private readonly resizeObserver: ResizeObserver;
  private readonly interaction: Interaction | undefined;
  private loader: TileLoader | undefined;
  private basemap: RasterLayer | undefined;
  private hillshade: RasterLayer | undefined;
  private animation: ((now: number) => boolean) | undefined;
  private hoverPoint: Point | undefined;
  private frameRequest = 0;
  private lastCamera = '';
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
    if (workers !== false && workers > 0 && typeof Worker !== 'undefined') {
      map.loader = new TileLoader(
        module,
        options.workerUrl ?? new URL('./decoder.worker.js', import.meta.url),
        workers,
        { ...DEFAULT_DECODERS, ...options.decoders },
        map.engine.textureFormats(),
      );
      map.engine.setTileLoader(map.loader);
    }
    return map;
  }

  private constructor(container: HTMLElement, options: GeoMapOptions) {
    this.canvas = document.createElement('canvas');
    Object.assign(this.canvas.style, { display: 'block', width: '100%', height: '100%', touchAction: 'none' });
    container.append(this.canvas);
    this.settings = {
      zoomRange: [options.minZoom ?? 0, options.maxZoom ?? 22],
      globe: (options.projection ?? 'globe') === 'globe',
      sky: options.sky ?? true,
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
    this.resize();
    this.interaction = (options.interactive ?? true) ? new Interaction(this) : undefined;
  }

  /** @internal The engine (replaced after a context loss) */
  get engine(): Engine {
    return this.#engine;
  }

  private createEngine(): Engine {
    const engine = new Engine(this.canvas, () => this.requestRender());
    const { zoomRange, globe, sky, background, globeColor } = this.settings;
    engine.setZoomRange(...zoomRange);
    engine.setGlobe(globe);
    engine.setSky(sky);
    engine.setBackground(...background);
    engine.setGlobeColor(...globeColor);
    if (this.loader) engine.setTileLoader(this.loader);
    return engine;
  }

  /** After a context loss: a new engine with the same camera, settings and layers (same handles, same order) */
  private restore(): void {
    const camera = this.getCamera();
    this.#engine.free();
    this.#engine = this.createEngine();
    this.lost = false;
    this.setCamera(camera);
    for (const layer of this.layers) layer.attach(this.#engine);
    this.resize();
  }

  /** @internal Forgets a removed layer */
  detach(layer: object): void {
    const index = this.layers.indexOf(layer as Layer);
    if (index >= 0) this.layers.splice(index, 1);
  }

  private add<L extends Layer>(layer: L): L {
    layer.attach(this.engine);
    this.layers.push(layer);
    this.requestRender();
    return layer;
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

  /** @internal */
  emit<K extends keyof MapEvents>(type: K, event: MapEvents[K]): void {
    const listeners: Set<(event: MapEvents[K]) => void> = this.listeners[type];
    for (const listener of listeners) listener(event);
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
    this.settings.globe = projection === 'globe';
    this.engine.setGlobe(this.settings.globe);
    this.requestRender();
  }

  /** Space behind the globe (stars, atmosphere halo) */
  setSky(enabled: boolean): void {
    this.settings.sky = enabled;
    this.engine.setSky(enabled);
    this.requestRender();
  }

  /** Color behind the map, `#rrggbb` */
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
  pick(point: Point): PickHit | undefined {
    return this.engine.pick(point[0], point[1]);
  }

  /** Screen point (CSS pixels) of a position, undefined behind the camera */
  project(lonLat: LonLat): Point | undefined {
    return this.engine.project(lonLat[0], lonLat[1]);
  }

  /** Position under a screen point (CSS pixels), undefined above the horizon */
  unproject(point: Point): LonLat | undefined {
    return this.engine.ground(point[0], point[1]);
  }

  // ── Layers ──

  /** Replaces the Géoplateforme basemap and hillshade, under the other layers */
  setBasemap(basemap: Basemap, hillshade = 0.4): void {
    this.basemap?.remove();
    this.hillshade?.remove();
    this.basemap = basemap === 'none' ? undefined : this.addRaster(BASEMAPS[basemap]);
    this.settings.globeColor = SEA[basemap];
    this.engine.setGlobeColor(...this.settings.globeColor);
    this.hillshade = hillshade > 0 ? this.addRaster({ ...HILLSHADE, opacity: hillshade }) : undefined;
    if (this.hillshade) this.toBottom(this.hillshade);
    if (this.basemap) this.toBottom(this.basemap);
  }

  /** Raster tiles in Web Mercator ({z}, {x}, {y} URL), over the layers already added */
  addRaster(options: RasterOptions): RasterLayer {
    const { url, minZoom = 0, maxZoom = 19, opacity = 1, saturation = 1 } = options;
    return this.add(new RasterLayer(this, { url, minZoom, maxZoom, opacity, saturation }));
  }

  /** Line network colored by a value per node; `layer.setValues` recolors it in one texture upload */
  addNetwork(options: NetworkOptions): NetworkLayer {
    const { nodes, edges, width = 2, max = 1800, contour = 0, opacity = 1 } = options;
    return this.add(new NetworkLayer(this, nodes, edges, { width, max, contour, opacity }));
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
    if (this.lost) return;
    const { clientWidth: width, clientHeight: height } = this.canvas;
    const ratio = window.devicePixelRatio || 1;
    this.canvas.width = Math.round(width * ratio);
    this.canvas.height = Math.round(height * ratio);
    this.engine.resize(width, height, ratio);
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
    for (const listeners of Object.values(this.listeners)) listeners.clear();
  }
}
