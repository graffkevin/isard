/**
 * The map: canvas, camera, layers, events, overlays, credits and the frame loop. Frames are drawn on demand only: an
 * idle map costs nothing. Hover events are coalesced to one per frame, so the values computed for a mouse position are
 * drawn in the same frame. The engine's loading is in engine-loader.ts, its settings from the options in settings.ts,
 * the camera animations in animation.ts.
 */
import { easeTo, flyTo, type Step } from './animation.ts';
import { BASEMAPS, basemapName, HILLSHADE, SEA } from './basemaps.ts';
import { AttributionControl, type Control, type ControlPosition } from './controls.ts';
import {
  destroyedEngine,
  type EngineClass,
  engineUrl,
  loadEngine,
  loadFullEngineLater,
  preloadFullEngine,
  startTileLoader,
} from './engine-loader.ts';
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
import { equirectangularGrid, gridLoader, gridZooms, type RemoteReprojection, resolveGrid } from './grid.ts';
import { Icons, type ImageOptions, type Pixels, readPixels } from './icons.ts';
import { Interaction } from './interaction.ts';
import {
  CustomLayer,
  type CustomLayerInterface,
  GroundLayer,
  type GroundOptions,
  type Layer,
  ModelLayer,
  type ModelOptions,
  NetworkLayer,
  RasterLayer,
  Tiles3dLayer,
} from './layers.ts';
import type { TileLoader } from './loader.ts';
import type { Positioned } from './overlay.ts';
import { loadPanoramas, type PanoramaHost, type PanoramaOptions, type Panoramas } from './panoramas.ts';
import { PointCloudLayer, type PointCloudOptions } from './pointclouds.ts';
import { type HeaderRules, headerRules, onHeaderRules } from './requests.ts';
import {
  atmosphereSettings,
  bodySettings,
  haloSettings,
  rgb,
  type Settings,
  type TerrainLoader,
  terrainSettings,
} from './settings.ts';
import { injectStyle } from './style.ts';
import { SymbolLayer, type SymbolOptions, symbolStyle } from './symbols.ts';
import { encodedTerrainLoader, type PngEncoding, type RemoteHeights } from './terrain.ts';
import type {
  AnimationOptions,
  AtmosphereOptions,
  Basemap,
  BasemapName,
  CelestialBody,
  FeatureHit,
  FirstPersonView,
  FlyToOptions,
  GeoMapOptions,
  LoadingMode,
  LonLat,
  MapEvents,
  NetworkOptions,
  Point,
  Projection,
  RasterOptions,
  SwipeOptions,
  TerrainModel,
  TerrainOptions,
  Tiles3dOptions,
  ViewshedOptions,
} from './types.ts';
import {
  BasemapWorkers,
  defaultWorkerCount,
  defaultWorkerUrl,
  type PmtilesOptions,
  pmtilesLayer,
  pmtilesRasterLoader,
  VECTOR_BASEMAPS,
  VectorTiles,
  type VectorTilesOptions,
} from './vectortiles.ts';
import type { Camera, Engine, PickHit, PickResult, RasterLayerStats } from './wasm/isard.js';

/** Reports an error without throwing it here: the console shows it, as an uncaught one */
function report(error: unknown): void {
  if (typeof reportError === 'function') reportError(error);
  else
    queueMicrotask(() => {
      throw error;
    });
}

type Listeners = { [K in keyof MapEvents]: Set<(event: MapEvents[K]) => void> };

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

const CORNERS: readonly ControlPosition[] = ['top-left', 'top-right', 'bottom-left', 'bottom-right'];

/** Frames the hover picking waits for its answer at most (usually one) */
const MAX_POLLS = 30;

/** The name of a basemap, checked: an unknown one throws */
function checkBasemap(basemap: Basemap): BasemapName {
  const name = basemapName(basemap);
  if (name !== 'none' && !(name in VECTOR_BASEMAPS) && !(name in BASEMAPS)) {
    throw new Error(`isard: unknown basemap '${basemap}'`);
  }
  return name;
}

/** A map (see `GeoMap.create`); layers take it as their host */
export class GeoMap {
  readonly canvas: HTMLCanvasElement;
  #glyphs: Glyphs | undefined;
  #icons: Icons | undefined;
  /** Layers new layers go under */
  private readonly onTop = new Set<Layer>();
  /** The vector basemap, if any: its tiles' workers and its labels */
  private vector: { tiles: VectorTiles; labels: SymbolLayer<unknown> } | undefined;
  private readonly glyphsUrl: string | undefined;
  private readonly basemapWorkerUrl: string | URL;
  /** Texture size of the vector basemap's tiles */
  private readonly basemapTileSize: number | undefined;
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
  private readonly tiles3dOptions: Pick<
    GeoMapOptions,
    'wasmUrl' | 'workers' | 'workerUrl' | 'decoders' | 'pointsWasmUrl' | 'assetsUrl'
  >;
  private gone = false;
  /** The map is of the Earth: a basemap by default */
  private readonly earth: boolean;
  /** The body, as the option named it */
  private readonly body: CelestialBody | undefined;
  private basemap: RasterLayer | undefined;
  private hillshade: RasterLayer | undefined;
  /** Called when the credits of the layers or relief change (the attribution controls) */
  private readonly attributionListeners = new Set<() => void>();
  /** Credits of the raster layers that have some (basemaps, hillshade, the Géoplateforme's rasters…) */
  private readonly rasterCredits = new Map<object, string>();
  private animation: Step | undefined;
  private hoverPoint: Point | undefined;
  /** Frame polling the engine for the answer of the hover picking, 0 if none */
  private hoverPoll = 0;
  /** Polls of the hover picking under way: given up after `MAX_POLLS` (an answer that never comes) */
  private hoverPolls = 0;
  /** What 'hoverfeature' looks for: within `radius` CSS pixels, in `layers` only if given */
  hoverPicking: { radius?: number; layers?: readonly Layer[] } = {};
  private frameRequest = 0;
  private lastCamera = '';
  /** The camera moved since the last 'moveend' */
  private moving = false;
  /** The last frame found everything loaded ('idle' sent) */
  private settled = false;
  private inFrame = false;
  /** The camera of the frame being drawn: what `getCamera` gives the custom layers while the engine is busy */
  private frameCamera: Camera | undefined;
  /** Vector tiles made by `vectorTiles()`, destroyed with the map */
  private readonly vectorSets = new Set<VectorTiles>();
  /** The workers drawing vector tiles, shared by every `VectorTiles` of the map; created on first use */
  private basemapPool: BasemapWorkers | undefined;
  /** Stops watching the device pixel ratio */
  private unwatchRatio: (() => void) | undefined;
  /** Stops following the page's request headers (`setRequestHeaders`) */
  private unwatchHeaders: () => void = () => undefined;
  /** Stops listening to the canvas's context loss and restoration */
  private readonly unwatchContext: () => void;

  /** The loaders of the relief by encoding, URL and floor: the same loader for the same tiles (the engine keeps its
   * tiles when only the exaggeration changes) */
  private readonly terrainLoaders = new Map<string, TerrainLoader>();

  /** Loads the engine (once per page) and creates a map filling `container` */
  static async create(container: HTMLElement, options: GeoMapOptions = {}): Promise<GeoMap> {
    // With 3D Tiles from the start when asked, or when only that engine's URL is given (as before the core engine)
    const full = options.tiles3d === true || (options.wasmUrl !== undefined && options.coreWasmUrl === undefined);
    // Before anything is created
    if (options.basemap !== undefined) checkBasemap(options.basemap);
    const kind = full ? 'full' : 'core';
    const bindings = await loadEngine(kind, engineUrl(kind, options));
    const map = new GeoMap(container, options, bindings.Engine, full ? bindings.module : undefined);
    if (options.tiles3d === 'preload' && !full) map.preloadTiles3d();
    return map;
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
    this.basemapWorkerUrl = options.basemapWorkerUrl ?? defaultWorkerUrl(options.assetsUrl);
    this.basemapTileSize = options.basemapTileSize;
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
    this.earth = (options.body ?? 'earth') === 'earth';
    this.body = options.body;
    const body = bodySettings(options.body);
    this.settings = {
      body,
      zoomRange: [options.minZoom ?? 0, options.maxZoom ?? 22],
      maxPitch: options.maxPitch ?? 70,
      globe: (options.projection ?? 'globe') === 'globe',
      sky: options.sky ?? true,
      atmosphere: atmosphereSettings(options.atmosphere),
      terrain: terrainSettings(options.terrain, options.body, body.radius, this.terrainLoader),
      loading: options.loading ?? 'immediate',
      background: rgb(options.background ?? (this.earth ? '#f2f2f0' : '#000000')),
      globeColor: this.earth ? SEA['osm-positron'] : rgb(body.color),
    };
    this.#engine = this.createEngine();
    // Once it has an engine: a map whose creation failed before is not left following them
    this.unwatchHeaders = onHeaderRules((rules) => sendHeaders(this.#engine, rules));
    // The browser may take the GPU away (driver reset, background tab on mobile): the map waits, then rebuilds
    const lost = (event: Event) => {
      event.preventDefault();
      this.lost = true;
      cancelAnimationFrame(this.frameRequest);
      this.frameRequest = 0;
    };
    const restored = () => this.restore();
    this.canvas.addEventListener('webglcontextlost', lost);
    this.canvas.addEventListener('webglcontextrestored', restored);
    this.unwatchContext = () => {
      this.canvas.removeEventListener('webglcontextlost', lost);
      this.canvas.removeEventListener('webglcontextrestored', restored);
    };
    const start: Camera = this.earth
      ? { center: [2.4246, 48.8457], zoom: 13, pitch: 0, bearing: 0 }
      : { center: [0, 0], zoom: 2, pitch: 0, bearing: 0 };
    this.setCamera({ ...start, ...options });
    this.setBasemap(options.basemap ?? (this.earth ? 'osm-positron' : 'none'), options.hillshade ?? 0);
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
    // The body first: the layers' shaders may take its size
    const { radius, flattening, atmosphere: air, relief } = this.settings.body;
    engine.setBody(radius, flattening, haloSettings(air), ...relief);
    sendHeaders(engine, headerRules());
    const { zoomRange, maxPitch, globe, sky, atmosphere, terrain, loading, background, globeColor } = this.settings;
    engine.setZoomRange(...zoomRange);
    engine.setMaxPitch(maxPitch);
    engine.setGlobe(globe);
    engine.setSky(sky);
    engine.setAtmosphere(atmosphere);
    engine.setTerrain(terrain?.url, terrain?.exaggeration ?? 1, terrain?.maxZoom, terrain?.geoid, terrain?.loader);
    engine.setImmediateLoading(loading === 'immediate');
    engine.setBackground(...background);
    engine.setGlobeColor(...globeColor);
    this.loader ??= this.startLoader(engine);
    if (this.loader) engine.setTileLoader(this.loader);
    return engine;
  }

  /** The decoding workers of 3D Tiles, with the engine that has them (they run its module) and unless turned off */
  private startLoader(engine: Engine): TileLoader | undefined {
    return this.tiles3dModule && startTileLoader(this.tiles3dModule, this.tiles3dOptions, engine);
  }

  /** A 3D Tiles layer or a model on the core engine: loads the engine with 3D Tiles, then rebuilds the map on it
   * (camera, settings and layers kept, as after a context loss); the layer is created then */
  private upgrade(): void {
    this.upgrading ??= loadFullEngineLater(engineUrl('full', this.tiles3dOptions)).then(
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
        if (this.gone) return;
        report(error);
        this.requestRender();
      },
    );
  }

  /** Downloads and compiles the engine with 3D Tiles once the map is idle (`tiles3d: 'preload'`), without running
   * it: the first 3D Tiles layer then only instantiates it */
  private preloadTiles3d(): void {
    const url = engineUrl('full', this.tiles3dOptions);
    const later = () => preloadFullEngine(url, () => !this.gone && !this.tiles3dModule);
    if (this.settled) later();
    else {
      const off = this.on('idle', () => {
        off();
        later();
      });
    }
  }

  /** After a context loss: a new engine with the same camera, settings and layers (same handles, same order) */
  private restore(): void {
    if (this.gone) return;
    this.cancelHoverPick();
    const camera = this.getCamera();
    this.#engine.free();
    this.#engine = this.createEngine();
    this.lost = false;
    this.setCamera(camera);
    this.#glyphs?.attach(this.#engine);
    this.#icons?.attach(this.#engine);
    const engine = this.#engine;
    // No layer refers to an id of the old engine while the others are created
    for (const layer of this.layers) layer.forgetEngine();
    for (const layer of this.layers) {
      // Still waiting for the engine with 3D Tiles (loading, or failed): created with it
      if (layer.needsFullEngine && !this.tiles3dModule) continue;
      // A layer that fails is reported and left out; the others are created
      try {
        layer.attach(engine);
      } catch (error) {
        report(error);
      }
    }
    // Links between layers, once every layer has its new id (buildings colored by a network above them)
    for (const layer of this.layers) {
      if (layer.id < 0) continue;
      try {
        layer.relink(engine);
      } catch (error) {
        report(error);
      }
    }
    this.applySwipe();
    // A street-level eye stays one in the new engine
    if (this.#firstPerson) this.setFirstPerson(this.#firstPerson);
    // What the old engine drew over the scene: the viewshed, the panoramas shown
    if (this.viewshed) this.setViewshed(this.viewshed);
    for (const set of this.panoramaSets) if (set.current) void set.show(set.current.index).catch(report);
    this.resize();
  }

  /** The street-level eye, while there is one (`setFirstPerson`) */
  #firstPerson: Required<FirstPersonView> | undefined;

  /** The street-level eye, if the camera is one */
  get firstPerson(): Readonly<Required<FirstPersonView>> | undefined {
    return this.#firstPerson;
  }

  /**
   * Puts the camera at street level: an eye `height` metres above the ground, looking `bearing` and `pitch`, the
   * drag turning it around (the wheel narrows or widens its field of view); undefined gives back the camera turning
   * around a point, above where the eye stood, facing the same way
   */
  setFirstPerson(view: FirstPersonView | undefined): void {
    if (!view) {
      const was = this.#firstPerson;
      this.#firstPerson = undefined;
      if (was) this.setCamera({ center: was.lonLat, zoom: 18, pitch: 60, bearing: was.bearing });
      return;
    }
    const eye = {
      height: 1.7,
      pitch: 90,
      fov: 75,
      ...view,
      lonLat: [view.lonLat[0], view.lonLat[1]] as LonLat,
    };
    eye.pitch = Math.max(1, Math.min(179, eye.pitch));
    eye.fov = Math.max(10, Math.min(140, eye.fov));
    eye.bearing = ((eye.bearing % 360) + 360) % 360;
    this.#firstPerson = eye;
    this.engine.setFirstPerson(eye.lonLat[0], eye.lonLat[1], eye.height, eye.bearing, eye.pitch, eye.fov);
    this.requestRender();
  }

  /** The swipe set (`setSwipe`), applied again with the layers' ids of a new engine */
  private swipe: SwipeOptions | undefined;

  /**
   * Compares two states of a place: `layers` shown on one side of a vertical line only, at `position` CSS pixels
   * from the left of the map (left of it by default, `side: 'right'` for the other), the layers beneath them on the
   * other side. Set again as the line moves (a slider, a drag); undefined stops
   */
  setSwipe(options: SwipeOptions | undefined): void {
    this.swipe = options && options.layers.length > 0 ? { ...options, layers: [...options.layers] } : undefined;
    this.applySwipe();
    this.requestRender();
  }

  private applySwipe(): void {
    const s = this.swipe;
    const ids = Uint32Array.from(
      (s?.layers ?? []).filter((l) => l.id >= 0),
      (l) => l.id,
    );
    this.engine.setSwipe(s && ids.length > 0 ? s.position : Number.NaN, s?.side === 'right', ids);
  }

  /** @internal Forgets a removed layer */
  detach(layer: object): void {
    this.unlist(layer);
    this.onTop.delete(layer as Layer);
    if (this.rasterCredits.delete(layer)) this.attributionChanged();
  }

  /** @internal The layer is on the map */
  hasLayer(layer: object): boolean {
    return !this.gone && this.layers.includes(layer as Layer);
  }

  /** Takes a layer out of the drawing order (to put it back elsewhere, or for good) */
  private unlist(layer: object): void {
    const index = this.layers.indexOf(layer as Layer);
    if (index >= 0) this.layers.splice(index, 1);
  }

  private add<L extends Layer>(layer: L): L {
    // On the core engine, a 3D Tiles layer or a model waits for the engine with 3D Tiles: it is created with the
    // others on it
    if (layer.needsFullEngine && !this.tiles3dModule) this.upgrade();
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
    this.unlist(handle);
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
    this.unlist(layer);
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
    return this.frameCamera ?? this.engine.camera();
  }

  setCamera(camera: Partial<Camera>): void {
    const c = { ...this.getCamera(), ...camera };
    this.engine.setCamera(c.center[0], c.center[1], c.zoom, c.pitch, c.bearing);
    this.requestRender();
  }

  /** Animated move to a camera: over `duration` milliseconds (or the options), ease-out by default */
  easeTo(camera: Partial<Camera>, options: number | AnimationOptions = 600): void {
    const from = this.getCamera();
    const to = { ...from, ...camera };
    this.animate(easeTo(() => this.engine, from, to, options));
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
    const viewport = Math.max(this.canvas.clientWidth, this.canvas.clientHeight);
    this.animate(flyTo(() => this.engine, from, to, viewport, options));
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

  /** Tints what can be seen from a point and what is hidden from it, within a range, all around or within a cone
   * (`direction`): the relief, extrusions, 3D Tiles and models hide what lies behind them (only what the map has
   * loaded for its view). Several observers (16 at most): what any of them sees is seen, what is hidden from all those
   * that reach it is hidden, in the colors and opacity of the first. Undefined or none removes it */
  /** The viewshed set, set again in a new engine */
  private viewshed: ViewshedOptions | readonly ViewshedOptions[] | undefined;

  setViewshed(options: ViewshedOptions | readonly ViewshedOptions[] | undefined): void {
    this.viewshed = options;
    let observers: readonly ViewshedOptions[] = [];
    if (options) observers = 'position' in options ? [options] : options;
    const [first] = observers;
    if (!first) {
      this.engine.clearViewshed();
      return;
    }
    this.engine.setViewshed(
      new Float64Array(
        observers.flatMap((o) => [
          o.position[0],
          o.position[1],
          o.height ?? 1.7,
          o.range ?? 1000,
          ...(o.direction === undefined
            ? [Number.NaN, 0, 0, 0]
            : [o.direction, o.tilt ?? 0, o.fov ?? 60, o.verticalFov ?? 40]),
        ]),
      ),
      new Float32Array(rgb(first.visibleColor ?? '#2f9e44')),
      new Float32Array(rgb(first.hiddenColor ?? '#e03131')),
      first.opacity ?? 0.7,
      new Float32Array(first.coneColor ? rgb(first.coneColor) : []),
    );
    this.requestRender();
  }

  /** Relief under the basemap: true for the body's terrain model (on the Earth the worldwide Terrain Tiles, NASA's
   * on the Moon and Mars), one of the Earth's models by name (`'ign'`), its options, or false; on another body, its
   * options with a `url` */
  setTerrain(terrain: boolean | TerrainModel | TerrainOptions): void {
    this.settings.terrain = terrainSettings(terrain, this.body, this.settings.body.radius, this.terrainLoader);
    const t = this.settings.terrain;
    this.engine.setTerrain(t?.url, t?.exaggeration ?? 1, t?.maxZoom, t?.geoid, t?.loader);
    this.attributionChanged();
    this.requestRender();
  }

  /** @internal The credits of the raster layers (the basemap first) and of the relief in use, each once */
  attributions(): string[] {
    const layers = this.layers.map((layer) => this.rasterCredits.get(layer));
    const credits = [...layers, this.settings.terrain?.attribution];
    return [...new Set(credits.filter((c): c is string => c !== undefined))];
  }

  /** @internal Calls `listener` when the credits change; returns what stops it */
  onAttributionChange(listener: () => void): () => void {
    this.attributionListeners.add(listener);
    return () => this.attributionListeners.delete(listener);
  }

  private attributionChanged(): void {
    for (const listener of this.attributionListeners) listener();
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
   * The feature under a screen point (CSS pixels), undefined if none: its layer id and index. Reads one pixel of an
   * offscreen picking pass, whatever the number of features, but waits for the GPU to finish the frame (a stall): for
   * clicks and one-off queries. On hover, listen to 'hoverfeature' instead, which picks without waiting.
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
    // The engine cannot pick (no request in flight): the poll under way would wait for nothing
    if (!this.engine.requestPick(point[0], point[1], radius, only)) {
      this.cancelHoverPick();
      return;
    }
    // A new request replaces the one in flight (in the engine too): one poll loop at most
    this.hoverPolls = 0;
    if (!this.hoverPoll) this.hoverPoll = requestAnimationFrame(this.pollHoverPick);
  }

  private readonly pollHoverPick = (): void => {
    this.hoverPoll = 0;
    if (this.lost) return;
    const result: PickResult | undefined = this.engine.pollPick();
    if (!result) {
      // The GPU is not done yet (usually a frame): asked again next frame, without drawing; given up at last
      this.hoverPolls += 1;
      if (this.hoverPolls < MAX_POLLS) this.hoverPoll = requestAnimationFrame(this.pollHoverPick);
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

  /** Screen point (CSS pixels) of a position on the ground (the relief), or `height` metres above it, undefined
   * behind the camera */
  project(lonLat: LonLat, height = 0): Point | undefined {
    return this.engine.project(lonLat[0], lonLat[1], height);
  }

  /** Position under a screen point (CSS pixels), undefined above the horizon */
  unproject(point: Point): LonLat | undefined {
    return this.engine.ground(point[0], point[1]);
  }

  /**
   * Street-level panoramas (oriented images of a vehicle's cameras, the iTowns immersive format): their positions
   * and calibrations loaded, none shown until `show`, which projects one onto the scene around it
   */
  async addPanoramas(options: PanoramaOptions): Promise<Panoramas> {
    // Drawn by the engine with 3D Tiles: loaded first if needed
    if (!this.tiles3dModule) {
      this.upgrade();
      await this.upgrading;
      if (!this.tiles3dModule) throw new Error('isard: panoramas need the engine with 3D Tiles, which failed to load');
    }
    const host: PanoramaHost = {
      setPanorama: (...args) => {
        this.engine.setPanorama(...args);
        this.requestRender();
      },
      setPanoramaImage: (index, image) => this.engine.setPanoramaImage(index, image),
      clearPanorama: () => this.engine.clearPanorama(),
      geoidHeight: (lon, lat) => this.engineClass.geoidHeight(lon, lat),
      firstPerson: () => this.firstPerson,
      setFirstPerson: (view) => this.setFirstPerson(view),
      elevationAt: (lonLat) => this.elevationAt(lonLat),
      listen: (click, key, wheel) => {
        const off = this.on('click', ({ lonLat }) => click(lonLat));
        const container = this.getContainer();
        // Before the map's own wheel (its zoom, the eye's field of view)
        const onWheel = (e: WheelEvent) => {
          e.preventDefault();
          e.stopImmediatePropagation();
          wheel(e);
        };
        container.addEventListener('keydown', key);
        container.addEventListener('wheel', onWheel, { capture: true, passive: false });
        return () => {
          off();
          container.removeEventListener('keydown', key);
          container.removeEventListener('wheel', onWheel, { capture: true });
        };
      },
    };
    const panoramas = await loadPanoramas(host, options);
    this.panoramaSets.add(panoramas);
    return panoramas;
  }

  /** The panorama sets, their panorama shown again in a new engine */
  private readonly panoramaSets = new Set<Panoramas>();

  /** Altitude of the relief at a position, in metres (exaggeration included): from the copy of its tiles the map
   * keeps, as fine as the tiles loaded there; undefined without relief or where none is loaded yet */
  elevationAt(lonLat: LonLat): number | undefined {
    return this.engine.elevationAt(lonLat[0], lonLat[1]);
  }

  /** Metres on the ground per CSS pixel at a screen point (default the center of the map), across the screen: for a
   * scale bar; undefined above the horizon */
  metersPerPixel(point?: Point): number | undefined {
    const [x, y] = point ?? [this.canvas.clientWidth / 2, this.canvas.clientHeight / 2];
    const a = this.unproject([x - 0.5, y]);
    const b = this.unproject([x + 0.5, y]);
    if (!a || !b) return undefined;
    // Haversine, on the mean radius of the body
    const rad = Math.PI / 180;
    const [dLat, dLon] = [(b[1] - a[1]) * rad, (b[0] - a[0]) * rad];
    const h = Math.sin(dLat / 2) ** 2 + Math.cos(a[1] * rad) * Math.cos(b[1] * rad) * Math.sin(dLon / 2) ** 2;
    return 2 * this.settings.body.radius * Math.asin(Math.min(1, Math.sqrt(h)));
  }

  /** Whether a double click zooms in (Shift: out), on by default: off while a drawing takes double clicks */
  setDoubleClickZoom(enabled: boolean): void {
    if (this.interaction) this.interaction.doubleClickZoom = enabled;
  }

  /** What is drawn under a screen point (CSS pixels), on the relief or standing on it (a roof, a wall, a mesh): its
   * position and its height above the relief, in metres; undefined where nothing was drawn. The last frame's scene is
   * drawn again and read back, waiting for the GPU (a stall): for clicks and drags */
  pickSurface(point: Point): { lonLat: LonLat; height: number } | undefined {
    const hit = this.engine.surface(point[0], point[1]);
    return hit && { lonLat: [hit[0], hit[1]], height: hit[2] };
  }

  /** The height above the relief, in metres, of the top of what stands at a position: a roof, a mesh, 0 on the bare
   * ground; undefined where nothing of the last frame is there. The scene is drawn from above into a single pixel and
   * read back: cheap, but it waits for the GPU */
  surfaceAt(lonLat: LonLat): number | undefined {
    return this.engine.surfaceAt(lonLat[0], lonLat[1]);
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

  /** Replaces the basemap and the IGN hillshade, under the other layers (hillshade by default: 0, none). The former
   * names of the Géoplateforme basemaps (`LegacyBasemap`) are taken under their new ones */
  setBasemap(basemap: Basemap, hillshade = 0): void {
    const name = checkBasemap(basemap);
    if (this.basemap && this.hasLayer(this.basemap)) this.basemap.remove();
    if (this.hillshade && this.hasLayer(this.hillshade)) this.hillshade.remove();
    // Its raster and labels go with it
    this.vector?.tiles.destroy();
    this.vector = undefined;
    const vector = VECTOR_BASEMAPS[name as keyof typeof VECTOR_BASEMAPS];
    if (vector) {
      const tiles = new VectorTiles(this, {
        style: vector,
        ...(this.basemapWorkerUrl ? { workerUrl: this.basemapWorkerUrl } : {}),
      });
      this.basemap = tiles.raster(this.basemapTileSize ? { size: this.basemapTileSize } : {});
      this.vector = { tiles, labels: tiles.labels() as SymbolLayer<unknown> };
    } else {
      this.basemap = name === 'none' ? undefined : this.addRaster(BASEMAPS[name as keyof typeof BASEMAPS]);
    }
    // Off the Earth, the body's own color
    if (this.earth) this.settings.globeColor = SEA[name];
    this.engine.setGlobeColor(...this.settings.globeColor);
    this.hillshade = hillshade > 0 ? this.addRaster({ ...HILLSHADE, opacity: hillshade }) : undefined;
    if (this.hillshade) this.toBottom(this.hillshade);
    if (this.basemap) this.toBottom(this.basemap);
    this.attributionChanged();
  }

  /** Raster tiles ({z}, {x}, {y} URL; Web Mercator, or reprojected from another `grid`: longitudes and latitudes,
   * Lambert-93, UTM…), over the layers already added */
  addRaster(options: RasterOptions & { before?: Layer }): RasterLayer {
    const { url, grid = 'mercator', opacity = 1, saturation = 1, size, before } = options;
    let { minZoom = 0, maxZoom = 19 } = options;
    // A PMTiles archive (`pmtiles://…`): its tiles read by range requests
    let loader = options.loader ?? pmtilesRasterLoader(url, size ?? 256);
    if (grid !== 'mercator' && !loader) {
      if (grid === 'equirectangular') {
        // `minZoom` and `maxZoom` are levels of the source: its level n has the pixel size of Mercator level n + 1 at
        // the equator
        loader = gridLoader(
          url,
          resolveGrid(equirectangularGrid(minZoom, maxZoom)),
          size ?? 256,
          this.remoteReprojection,
        );
        [minZoom, maxZoom] = [minZoom + 1, maxZoom + 1];
      } else {
        const resolved = resolveGrid(grid);
        const zooms = gridZooms(resolved);
        minZoom = options.minZoom ?? zooms.minZoom;
        maxZoom = options.maxZoom ?? zooms.maxZoom;
        loader = gridLoader(url, resolved, size ?? 256, this.remoteReprojection);
      }
    }
    const layer = new RasterLayer(this, { url, minZoom, maxZoom, opacity, saturation, size, loader });
    this.place(layer, before);
    if (options.attribution) {
      this.rasterCredits.set(layer, options.attribution);
      this.attributionChanged();
    }
    return layer;
  }

  /**
   * A PMTiles archive (v3) read by range requests: a raster layer of its images, or vector tiles drawn with `style` (by
   * default a plain style of the archive's layers) with their labels. Its header decides which unless `kind` says;
   * `url` with or without `pmtiles://`, relative to the page, or a `blob:` URL. `addRaster({ url: 'pmtiles://…' })` and
   * a style source `pmtiles://…` in `vectorTiles` read archives too
   */
  addPmtiles(url: string, options: PmtilesOptions = {}): Promise<RasterLayer | VectorTiles> {
    return pmtilesLayer(this, url, options);
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

  /** @internal The icons of the symbol layers: the application's images and the vector basemaps' sprites */
  get icons(): Icons {
    if (!this.#icons) {
      const gl = this.canvas.getContext('webgl2');
      const max = gl?.getParameter(gl.MAX_TEXTURE_SIZE);
      this.#icons = new Icons(typeof max === 'number' ? max : undefined);
      this.#icons.attach(this.engine);
    }
    return this.#icons;
  }

  /**
   * An image for the symbol layers' icons (`icon: name`), as MapLibre's `addImage`: its pixels (an image, a canvas,
   * `ImageData`, or RGBA bytes with their size), `pixelRatio` 2 for an image drawn at twice its CSS size; `sdf` for a
   * signed distance field drawn in the `iconColor`; `stretchX`, `stretchY` and `content` for an icon stretched around its
   * text. An image of the same name is replaced
   */
  addImage(
    name: string,
    image: Pixels | (CanvasImageSource & { width: number; height: number }),
    options: ImageOptions = {},
  ): void {
    const pixels = 'data' in image ? image : readPixels(image);
    if (!pixels) throw new Error(`isard: the image ${name} has no pixels`);
    this.icons.add(name, pixels, options);
  }

  hasImage(name: string): boolean {
    return this.#icons?.has(name) ?? false;
  }

  removeImage(name: string): void {
    this.#icons?.remove(name);
  }

  /**
   * Vector tiles with a MapLibre style (OpenFreeMap's Positron by default): `tiles.raster(…)` draws its fills and lines
   * (or a subset) as a raster layer, `tiles.labels(…)` its labels. `setBasemap('osm-positron')` does both for the
   * basemap.
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

  /** @internal Forgets a destroyed `VectorTiles` */
  dropVectorTiles(tiles: VectorTiles): void {
    this.vectorSets.delete(tiles);
  }

  /**
   * @internal The workers for a `VectorTiles`: the map's shared pool, or a pool of its own (`own`, to stop with it)
   * when it asks for another count or URL
   */
  basemapWorkers(count?: number, url?: string | URL): { pool: BasemapWorkers; own: boolean } {
    const sharedUrl = this.basemapWorkerUrl;
    const shared = defaultWorkerCount();
    const workers = Math.max(1, count ?? shared);
    const workerUrl = url ?? sharedUrl;
    if (workers === shared && String(workerUrl) === String(sharedUrl)) {
      this.basemapPool ??= new BasemapWorkers(shared, sharedUrl);
      return { pool: this.basemapPool, own: false };
    }
    return { pool: new BasemapWorkers(workers, workerUrl), own: true };
  }

  /**
   * The workers decoding the relief and reprojecting rasters: the map's pool, unless the map has no workers
   * (`workers: false` or 0, or none in this browser): that is done in the page then
   */
  private decodingPool(): BasemapWorkers | undefined {
    const { workers } = this.tiles3dOptions;
    if (this.gone || workers === false || (workers !== undefined && workers <= 0)) return undefined;
    if (typeof Worker === 'undefined' || typeof OffscreenCanvas === 'undefined') return undefined;
    return this.basemapWorkers().pool;
  }

  private readonly remoteHeights: RemoteHeights = (url, encoding, floor, tile, signal) =>
    this.decodingPool()?.heights(url, encoding, floor, tile, signal, this);

  private readonly remoteReprojection: RemoteReprojection = (job, tile, signal) =>
    this.decodingPool()?.reproject(job, tile, signal, this);

  /** The loader of the relief tiles of `url` in a PNG encoding (one per URL, encoding and floor) */
  private readonly terrainLoader = (url: string, encoding: PngEncoding, floor?: number): TerrainLoader => {
    const key = `${encoding} ${floor ?? ''} ${url}`;
    let loader = this.terrainLoaders.get(key);
    if (!loader) {
      loader = encodedTerrainLoader(url, encoding, floor, this.remoteHeights);
      this.terrainLoaders.set(key, loader);
    }
    return loader;
  };

  /** Adds a layer, under `before` if given */
  private place<L extends Layer>(layer: L, before: Layer | undefined): L {
    this.add(layer);
    if (before) this.moveLayer(layer, before);
    return layer;
  }

  /** Textured meshes in 3D Tiles (photogrammetry, buildings), drawn with depth over the basemap */
  addTiles3d(options: Tiles3dOptions): Tiles3dLayer {
    const { url, maxScreenSpaceError = 16, opacity = 1, pointSize, color, geoid = false } = options;
    return this.add(
      new Tiles3dLayer(this, url, geoid, {
        maxScreenSpaceError,
        opacity,
        ...(pointSize !== undefined ? { pointSize } : {}),
        ...(color !== undefined ? { color } : {}),
      }),
    );
  }

  /**
   * A point cloud (see `PointCloudOptions`): a COPC file read by range requests (IGN's LiDAR HD…), or 3D Tiles
   * points; with depth, on the engine with 3D Tiles; under `before` if given
   */
  addPointCloud(options: PointCloudOptions & { before?: Layer }): PointCloudLayer {
    return this.place(new PointCloudLayer(this, options), options.before);
  }

  /** A glTF model drawn at positions (see `ModelOptions`), with depth; on the engine with 3D Tiles, as they are */
  addModels(options: ModelOptions & { before?: Layer }): ModelLayer {
    return this.place(new ModelLayer(this, options), options.before);
  }

  /**
   * The page's own WebGL drawing at its place among the layers, in every frame (see `CustomLayerInterface`): raw
   * WebGL, three.js… with the matrices of the frame (`frame.matrixAt`); under `before` if given
   */
  addCustomLayer(layer: CustomLayerInterface, options: { before?: Layer } = {}): CustomLayer {
    const custom = new CustomLayer(
      this,
      layer,
      this.canvas,
      this,
      (lon, lat, altitude) => this.engineClass.customMatrix(lon, lat, altitude),
      () => this.getCamera(),
    );
    return this.place(custom, options.before);
  }

  // ── Frames ──

  /** Asks for a frame; several calls before the next frame draw it once */
  requestRender(): void {
    // Changes made by listeners during a frame are drawn by that frame; nothing is drawn without a context
    if (this.inFrame || this.lost) return;
    this.frameRequest ||= requestAnimationFrame((now) => this.frame(now));
  }

  /** @internal Runs `step` every frame until it returns false; replaces the running animation */
  animate(step: Step | undefined): void {
    // A flight cut short no longer needs its destination
    if (this.animation) this.engine.clearPreload();
    this.animation = step;
    this.requestRender();
  }

  /** @internal The pointer moved over the map (undefined: it left). Without 'hover' and 'hoverfeature' listeners,
   * nothing is drawn: a still map stays idle under a moving mouse */
  hover(point: Point | undefined): void {
    if (!point) {
      this.hoverPoint = undefined;
      this.cancelHoverPick();
      return;
    }
    if (this.listeners.hover.size === 0 && this.listeners.hoverfeature.size === 0) return;
    this.hoverPoint = point;
    this.requestRender();
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
      // The position under the pointer for its listeners only (a 'hoverfeature' alone does not need it)
      if (this.listeners.hover.size > 0) this.emit('hover', { point, lonLat: this.unproject(point) });
    }
    const t0 = performance.now();
    // The engine asks for the next frame while it draws (uploads past the frame budget, fades): not ignored
    this.inFrame = false;
    // As the engine has it now (a listener may have moved it), for the custom layers drawn in the frame
    this.frameCamera = this.layers.some((layer) => layer instanceof CustomLayer) ? this.engine.camera() : undefined;
    let loading: boolean;
    try {
      loading = this.engine.render();
    } finally {
      this.frameCamera = undefined;
    }
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
    if (this.gone) return;
    this.gone = true;
    // A context restored later rebuilds nothing
    this.unwatchContext();
    const camera = this.getCamera();
    cancelAnimationFrame(this.frameRequest);
    this.cancelHoverPick();
    this.vector?.tiles.destroy();
    for (const tiles of [...this.vectorSets]) tiles.destroy();
    this.vectorSets.clear();
    this.basemapPool?.terminate();
    this.basemapPool = undefined;
    this.unwatchRatio?.();
    this.unwatchRatio = undefined;
    this.unwatchHeaders();
    this.resizeObserver.disconnect();
    this.interaction?.destroy();
    this.loader?.destroy();
    // Custom layers free their WebGL objects, models their data, while the context is there
    for (const layer of this.layers) layer.dispose();
    this.engine.free();
    // Work still under way (labels being shaped, tiles arriving, timers) may call the engine after this: it finds one
    // that does nothing, and nothing is drawn again
    this.#engine = destroyedEngine(camera);
    // Glyph ranges still arriving upload to it, not to the freed engine
    this.#glyphs?.attach(this.#engine);
    this.#icons?.attach(this.#engine);
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

/** The page's request headers to an engine (its own requests: raster tiles, tilesets, point clouds) */
function sendHeaders(engine: Engine, rules: HeaderRules): void {
  engine.setRequestHeaders(
    rules.map(([prefix]) => prefix),
    Uint32Array.from(rules, ([, headers]) => Object.keys(headers).length),
    rules.flatMap(([, headers]) => Object.entries(headers).flat()),
  );
}
