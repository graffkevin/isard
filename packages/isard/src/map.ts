/**
 * The map: canvas, camera, layers, events, overlays, credits and the frame loop. Frames are drawn on demand only: an
 * idle map costs nothing. Hover events are coalesced to one per frame, so the values computed for a mouse position are
 * drawn in the same frame. The engine's loading is in engine-loader.ts, its settings from the options in settings.ts,
 * the camera animations in animation.ts; the parts of the map with a state of their own (the drawing order of the
 * layers, the overlays and controls, the frame loop, the hover picking, the workers…) in map/.
 */

import { SEA } from './basemap/basemaps.ts';
import {
  type BasemapWorkers,
  defaultWorkerUrl,
  type PmtilesOptions,
  pmtilesLayer,
  VectorTiles,
  type VectorTilesOptions,
} from './basemap/vectortiles.ts';
import { onHeaderRules } from './engine/requests.ts';
import { destroyedEngine, type EngineClass, engineUrl, loadEngine } from './engine-loader.ts';
import { IGN_PRESETS } from './geoplateforme.ts';
import { ExtrusionLayer, type ExtrusionOptions, extrusionStyle } from './layers/extrusions.ts';
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
} from './layers/features.ts';
import type { Glyphs } from './layers/glyphs.ts';
import type { Icons, ImageOptions, Pixels } from './layers/icons.ts';
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
} from './layers/layers.ts';
import { loadPanoramas, type PanoramaOptions, type Panoramas } from './layers/panoramas.ts';
import { PointCloudLayer, type PointCloudOptions } from './layers/pointclouds.ts';
import { SymbolLayer, type SymbolOptions, symbolStyle } from './layers/symbols.ts';
import { BasemapLayers } from './map/basemap-layers.ts';
import { createCanvas, labelCanvas, watchContext, watchPixelRatio } from './map/canvas.ts';
import { checkBasemap, report, validateNetwork } from './map/checks.ts';
import { Credits } from './map/credits.ts';
import { mapSettings, sendHeaders } from './map/engine-settings.ts';
import { MapEngines } from './map/engines.ts';
import { MapListeners } from './map/events.ts';
import { FrameLoop } from './map/frame-loop.ts';
import { HoverPick, type HoverPicking } from './map/hover-pick.ts';
import { addSpec, type LayerSpec } from './map/layer-spec.ts';
import { LayerStack } from './map/layer-stack.ts';
import { OverlayHost } from './map/overlay-host.ts';
import { panoramaHost } from './map/panorama-host.ts';
import { rasterSource } from './map/raster-source.ts';
import { SymbolAtlases } from './map/symbol-atlases.ts';
import { applyViewshed } from './map/viewshed.ts';
import { MapWorkers } from './map/workers.ts';
import { atmosphereSettings, rgb, type Settings, terrainSettings } from './settings.ts';
import type {
  AnimationOptions,
  AtmosphereOptions,
  Basemap,
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
import { AttributionControl, type Control, type ControlPosition } from './ui/controls.ts';
import { Interaction } from './ui/interaction.ts';
import type { Positioned } from './ui/overlay.ts';
import { easeTo, flyTo, type Step } from './util/animation.ts';
import type { Camera, Engine, PickHit, RasterLayerStats } from './wasm/isard.js';

export type { LayerSpec } from './map/layer-spec.ts';

/** A map (see `GeoMap.create`); layers take it as their host */
export class GeoMap {
  readonly canvas: HTMLCanvasElement;
  #engine: Engine;
  /** The engine class (core, or with 3D Tiles) and the engine with 3D Tiles loaded when needed */
  private readonly engines: MapEngines;
  /** The layers in drawing order and those kept on top */
  private readonly stack = new LayerStack(() => this.#engine);
  /** The basemap and the hillshade */
  private readonly basemaps: BasemapLayers;
  /** The glyphs and icons of the symbol layers */
  private readonly atlases: SymbolAtlases;
  /** The workers of the vector tiles, the relief and the reprojected rasters */
  private readonly workers: MapWorkers;
  private readonly container: HTMLElement;
  /** The canvas in the container, the markers and popups over it, the controls in its corners */
  private readonly overlays: OverlayHost;
  private readonly settings: Settings;
  /** The WebGL context is lost: nothing is drawn until it is restored */
  private lost = false;
  private gone = false;
  private readonly listeners = new MapListeners();
  /** Frames drawn on demand, with their events */
  private readonly frames: FrameLoop;
  private readonly resizeObserver: ResizeObserver;
  private readonly interaction: Interaction | undefined;
  /** The map is of the Earth: a basemap by default */
  private readonly earth: boolean;
  /** The body, as the option named it */
  private readonly body: CelestialBody | undefined;
  /** Credits of the raster layers that have some, and the attribution controls told when they change */
  private readonly credits = new Credits();
  /** The picking for 'hoverfeature', its answer emitted on a later frame */
  private readonly hoverPick = new HoverPick(
    () => this.engine,
    () => this.lost,
    (result) =>
      this.emit('hoverfeature', {
        point: [result.x, result.y],
        lonLat: result.lonLat,
        hit: result.hit && this.featureHit(result.hit),
      }),
  );
  /** What 'hoverfeature' looks for: within `radius` CSS pixels, in `layers` only if given */
  hoverPicking: HoverPicking = {};
  /** Vector tiles made by `vectorTiles()`, destroyed with the map */
  private readonly vectorSets = new Set<VectorTiles>();
  /** Stops watching the device pixel ratio */
  private readonly unwatchRatio: () => void;
  /** Stops following the page's request headers (`setRequestHeaders`) */
  private unwatchHeaders: () => void = () => undefined;
  /** Stops listening to the canvas's context loss and restoration */
  private readonly unwatchContext: () => void;

  /** Loads the engine (once per page) and creates a map filling `container` */
  static async create(container: HTMLElement, given: GeoMapOptions = {}): Promise<GeoMap> {
    // A preset under the options given
    const options: GeoMapOptions = given.preset ? { ...IGN_PRESETS[given.preset], ...given } : given;
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
    this.engines = new MapEngines(engineClass, tiles3dModule, options, () => this.gone);

    const workerUrl = options.basemapWorkerUrl ?? defaultWorkerUrl(options.assetsUrl);

    this.workers = new MapWorkers(workerUrl, options.workers, () => this.gone, this);
    this.basemaps = new BasemapLayers(this, this.stack, workerUrl, options.basemapTileSize);
    this.container = container;
    this.canvas = createCanvas();
    this.atlases = new SymbolAtlases(this.canvas, options.glyphs, () => this.#engine);
    this.overlays = new OverlayHost(container, this.canvas, this);
    this.earth = (options.body ?? 'earth') === 'earth';
    this.body = options.body;
    this.settings = mapSettings(options, this.earth, this.workers.terrainLoader);
    this.frames = new FrameLoop(
      {
        engine: () => this.engine,
        camera: () => this.getCamera(),
        lost: () => this.lost,
        interacting: () => this.interaction?.active === true,
        customLayers: () => this.layers.some((layer) => layer instanceof CustomLayer),
        upgrading: () => this.engines.upgrading !== undefined,
        drawn: (hovered) => {
          // The picking pass sees the frame just drawn; its answer comes on a later frame
          if (hovered && this.listeners.has('hoverfeature')) this.hoverPick.request(hovered, this.hoverPicking);
          // Overlays follow the frame just drawn
          this.overlays.positionAll();
        },
      },
      this.listeners,
    );
    this.#engine = this.createEngine();
    // Once it has an engine: a map whose creation failed before is not left following them
    this.unwatchHeaders = onHeaderRules((rules) => sendHeaders(this.#engine, rules));
    // The browser may take the GPU away (driver reset, background tab on mobile): the map waits, then rebuilds
    this.unwatchContext = watchContext(
      this.canvas,
      () => {
        this.lost = true;
        this.frames.cancel();
      },
      () => this.restore(),
    );

    const start: Camera = this.earth
      ? { center: [2.4246, 48.8457], zoom: 13, pitch: 0, bearing: 0 }
      : { center: [0, 0], zoom: 2, pitch: 0, bearing: 0 };

    this.setCamera({ ...start, ...options });
    this.setBasemap(options.basemap ?? (this.earth ? 'osm-positron' : 'none'), options.hillshade ?? 0);
    this.resizeObserver = new ResizeObserver(() => this.resize());
    this.resizeObserver.observe(container);
    this.unwatchRatio = watchPixelRatio(() => this.resize());
    this.resize();

    const interactive = options.interactive ?? true;
    const keyboard = interactive && (options.keyboard ?? true);

    this.interaction = interactive ? new Interaction(this, { keyboard }) : undefined;
    labelCanvas(this.canvas, keyboard, options.ariaLabel ?? 'Map');

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

  /** Layers in drawing order (bottom first) */
  private get layers(): readonly Layer[] {
    return this.stack.layers;
  }

  /** Layers new layers go under */
  private get onTop(): Set<Layer> {
    return this.stack.onTop;
  }

  private createEngine(): Engine {
    return this.engines.create(this.canvas, () => this.requestRender(), this.settings);
  }

  /** A 3D Tiles layer or a model on the core engine: loads the engine with 3D Tiles, then rebuilds the map on it
   * (camera, settings and layers kept, as after a context loss); the layer is created then */
  private upgrade(): void {
    this.engines.upgrade(
      () => {
        // A lost context: the engine is rebuilt on it once restored
        if (!this.lost) this.restore();
      },
      (error) => {
        // The 3D Tiles layers stay empty; the next one added tries again
        report(error);
        this.requestRender();
      },
    );
  }

  /** Downloads and compiles the engine with 3D Tiles once the map is idle (`tiles3d: 'preload'`), without running
   * it: the first 3D Tiles layer then only instantiates it */
  private preloadTiles3d(): void {
    if (this.frames.settled) this.engines.preload();
    else {
      const off = this.on('idle', () => {
        off();
        this.engines.preload();
      });
    }
  }

  /** After a context loss: a new engine with the same camera, settings and layers (same handles, same order) */
  private restore(): void {
    if (this.gone) return;
    this.hoverPick.cancel();

    const camera = this.getCamera();

    this.#engine.free();
    this.#engine = this.createEngine();
    this.lost = false;
    this.setCamera(camera);
    this.atlases.attach(this.#engine);
    // Those still waiting for the engine with 3D Tiles (loading, or failed) are created with it
    this.stack.reattach(this.#engine, (layer) => layer.needsFullEngine && !this.engines.tiles3d);
    this.applySwipe();
    // A street-level eye stays one in the new engine
    if (this.#firstPerson) this.setFirstPerson(this.#firstPerson);
    // What the old engine drew over the scene: the viewshed, the panoramas shown
    if (this.viewshed) this.setViewshed(this.viewshed);
    this.panoramaSets.forEach((set) => {
      if (set.current) void set.show(set.current.index).catch(report);
    });
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
    this.stack.forget(layer);
    this.credits.forget(layer);
  }

  /** @internal The layer is on the map */
  hasLayer(layer: object): boolean {
    return !this.gone && this.stack.has(layer);
  }

  private addLayer<L extends Layer>(layer: L): L {
    // On the core engine, a 3D Tiles layer or a model waits for the engine with 3D Tiles: it is created with the
    // others on it
    if (layer.needsFullEngine && !this.engines.tiles3d) this.upgrade();
    else layer.attach(this.engine);
    // Under the layers kept on top (the basemap's labels)
    this.stack.add(layer);
    this.requestRender();

    return layer;
  }

  /** @internal Keeps a layer above the layers added after it (they go under it), as the labels of a basemap */
  keepOnTop(layer: Layer): void {
    this.onTop.add(layer);
  }

  /** @internal Moves a layer under `before`, or on top of all */
  moveLayer(layer: object, before: object | undefined): void {
    this.stack.move(layer, before);
    this.requestRender();
  }

  /** @internal See `LayerHost.replaceLayer` */
  replaceLayer(layer: object, old: number): void {
    this.stack.replace(layer, old);
  }

  // ── Events ──

  /** Listens to an event; returns the function that stops listening */
  on<K extends keyof MapEvents>(type: K, listener: (event: MapEvents[K]) => void): () => void {
    return this.listeners.on(type, listener);
  }

  /** @internal Calls the listeners; one that throws is reported and does not stop the others nor the frame */
  emit<K extends keyof MapEvents>(type: K, event: MapEvents[K]): void {
    this.listeners.emit(type, event);
  }

  // ── Camera ──

  /**
   * The camera's methods together: `get`, `set`, `easeTo`, `flyTo`, `bounds`, `firstPerson` and `setFirstPerson`
   * (street level), `project`, `unproject`; the same as the map's own (`getCamera`, `setCamera`…)
   */
  readonly camera = {
    get: (): Camera => this.getCamera(),
    set: (camera: Partial<Camera>): void => this.setCamera(camera),
    easeTo: (camera: Partial<Camera>, options?: number | AnimationOptions): void => this.easeTo(camera, options),
    flyTo: (camera: Partial<Camera>, options?: FlyToOptions): void => this.flyTo(camera, options),
    bounds: (): [west: number, south: number, east: number, north: number] => this.getBounds(),
    firstPerson: (): Readonly<Required<FirstPersonView>> | undefined => this.firstPerson,
    setFirstPerson: (view: FirstPersonView | undefined): void => this.setFirstPerson(view),
    project: (lonLat: LonLat, height?: number): Point | undefined => this.project(lonLat, height),
    unproject: (point: Point): LonLat | undefined => this.unproject(point),
  } as const;

  /**
   * What lies under a point or at a position, together: `feature` (`pickFeature`), `surface` (`pickSurface`),
   * `surfaceAt`, `elevationAt`, `metersPerPixel`
   */
  readonly query = {
    feature: (point: Point, options?: { radius?: number; layers?: readonly Layer[] }): FeatureHit | undefined =>
      this.pickFeature(point, options),
    surface: (point: Point): { lonLat: LonLat; height: number } | undefined => this.pickSurface(point),
    surfaceAt: (lonLat: LonLat): number | undefined => this.surfaceAt(lonLat),
    elevationAt: (lonLat: LonLat): number | undefined => this.elevationAt(lonLat),
    metersPerPixel: (point?: Point): number | undefined => this.metersPerPixel(point),
  } as const;

  /** What is drawn over the scene, together: `viewshed` (`setViewshed`), `swipe` (`setSwipe`) */
  readonly effects = {
    viewshed: (options: ViewshedOptions | readonly ViewshedOptions[] | undefined): void => this.setViewshed(options),
    swipe: (options: SwipeOptions | undefined): void => this.setSwipe(options),
  } as const;

  getCamera(): Camera {
    return this.frames.camera ?? this.engine.camera();
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

  /** The viewshed set, set again in a new engine */
  private viewshed: ViewshedOptions | readonly ViewshedOptions[] | undefined;

  /** Tints what can be seen from a point and what is hidden from it, within a range, all around or within a cone
   * (`direction`): the relief, extrusions, 3D Tiles and models hide what lies behind them (only what the map has
   * loaded for its view). Several observers (16 at most): what any of them sees is seen, what is hidden from all those
   * that reach it is hidden, in the colors and opacity of the first. Undefined or none removes it */
  setViewshed(options: ViewshedOptions | readonly ViewshedOptions[] | undefined): void {
    this.viewshed = options;
    if (applyViewshed(this.engine, options)) this.requestRender();
  }

  /** Relief under the basemap: true for the body's terrain model (on the Earth the worldwide Terrain Tiles, NASA's
   * on the Moon and Mars), one of the Earth's models by name (`'ign'`), its options, or false; on another body, its
   * options with a `url` */
  setTerrain(terrain: boolean | TerrainModel | TerrainOptions): void {
    this.settings.terrain = terrainSettings(terrain, this.body, this.settings.body.radius, this.workers.terrainLoader);

    const t = this.settings.terrain;

    this.engine.setTerrain(t?.url, t?.exaggeration ?? 1, t?.maxZoom, t?.geoid, t?.loader);
    this.credits.changed();
    this.requestRender();
  }

  /** @internal The credits of the raster layers (the basemap first) and of the relief in use, each once */
  attributions(): string[] {
    return this.credits.of(this.layers, this.settings.terrain?.attribution);
  }

  /** @internal Calls `listener` when the credits change; returns what stops it */
  onAttributionChange(listener: () => void): () => void {
    return this.credits.onChange(listener);
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
    if (!this.engines.tiles3d) {
      this.upgrade();
      await this.engines.upgrading;
      if (!this.engines.tiles3d)
        throw new Error('isard: panoramas need the engine with 3D Tiles, which failed to load');
    }

    const host = panoramaHost(this, (lon, lat) => this.engines.geoidHeight(lon, lat));
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
    this.overlays.addControl(control, position);

    return this;
  }

  removeControl(control: Control): this {
    this.overlays.removeControl(control);

    return this;
  }

  /** @internal Shows a marker or popup, placed after every frame from now on */
  attachOverlay(item: Positioned, element: HTMLElement): void {
    this.overlays.attach(item, element);
  }

  /** @internal */
  detachOverlay(item: Positioned, element: HTMLElement): void {
    this.overlays.detach(item, element);
  }

  /** @internal Places one overlay now (it moved), without drawing a frame */
  positionOverlay(item: Positioned): void {
    this.overlays.position(item);
  }

  // ── Layers ──

  /** Replaces the basemap and the IGN hillshade, under the other layers (hillshade by default: 0, none). The former
   * names of the Géoplateforme basemaps throw, naming the new ones */
  setBasemap(basemap: Basemap, hillshade = 0): void {
    const name = checkBasemap(basemap);

    this.basemaps.replace(name);
    // Off the Earth, the body's own color
    if (this.earth) this.settings.globeColor = SEA[name];
    this.engine.setGlobeColor(...this.settings.globeColor);
    this.basemaps.addHillshade(hillshade);
    this.credits.changed();
  }

  /** Raster tiles ({z}, {x}, {y} URL; Web Mercator, or reprojected from another `grid`: longitudes and latitudes,
   * Lambert-93, UTM…), over the layers already added */
  addRaster(options: RasterOptions & { before?: Layer }): RasterLayer {
    const { url, opacity = 1, saturation = 1, size, before } = options;
    const { loader, minZoom, maxZoom } = rasterSource(options, this.workers.remoteReprojection);
    const layer = new RasterLayer(this, { url, minZoom, maxZoom, opacity, saturation, size, loader });

    this.place(layer, before);
    if (options.attribution) this.credits.set(layer, options.attribution);

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

    return this.addLayer(new NetworkLayer(this, { nodes, edges, lines }, costs, style, edgeWidths));
  }

  /**
   * A layer from its description, as an application's configuration gives it: `{ type, ...options }`, the options of
   * the `add…` method of that type (`'lines'` for `addLines`, `'tiles3d'` for `addTiles3d`…); `before` to go under
   * another layer. The layer's `ready` resolves once the map has drawn it
   */
  add<P = Record<string, unknown>>(spec: LayerSpec<P>): Layer {
    return addSpec<P>(this, spec);
  }

  /** @internal A layer made anew from its description, in the place of `old` in the drawing order (old removed) */
  rebuildLayer<P = Record<string, unknown>>(old: Layer, spec: LayerSpec<P>): Layer {
    const index = this.layers.indexOf(old);
    const above = index >= 0 ? this.layers[index + 1] : undefined;
    const layer = this.add(spec);

    if (!('before' in spec && spec.before) && index >= 0) this.moveLayer(layer, above);
    old.remove();

    return layer;
  }

  /** @internal Resolves once the map is idle (everything in view loaded and drawn) */
  whenIdle(): Promise<void> {
    if (this.gone) return Promise.resolve();

    return new Promise((resolve) => {
      const off = this.on('idle', () => {
        off();
        resolve();
      });

      this.requestRender();
    });
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
    return this.atlases.glyphs;
  }

  /** @internal The icons of the symbol layers: the application's images and the vector basemaps' sprites */
  get icons(): Icons {
    return this.atlases.icons;
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
    this.atlases.addImage(name, image, options);
  }

  hasImage(name: string): boolean {
    return this.atlases.hasImage(name);
  }

  removeImage(name: string): void {
    this.atlases.removeImage(name);
  }

  /**
   * Vector tiles with a MapLibre style (OpenFreeMap's Positron by default): `tiles.raster(…)` draws its fills and lines
   * (or a subset) as a raster layer, `tiles.labels(…)` its labels. `setBasemap('osm-positron')` does both for the
   * basemap.
   */
  vectorTiles(options: VectorTilesOptions = {}): VectorTiles {
    const tiles = new VectorTiles(this, {
      ...(this.workers.url ? { workerUrl: this.workers.url } : {}),
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
    return this.workers.pool(count, url);
  }

  /** Adds a layer, under `before` if given */
  private place<L extends Layer>(layer: L, before: Layer | undefined): L {
    this.addLayer(layer);
    if (before) this.moveLayer(layer, before);

    return layer;
  }

  /** Textured meshes in 3D Tiles (photogrammetry, buildings), drawn with depth over the basemap */
  addTiles3d(options: Tiles3dOptions): Tiles3dLayer {
    const { url, maxScreenSpaceError = 16, opacity = 1, pointSize, color, geoid = false } = options;

    return this.addLayer(
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
      (lon, lat, altitude) => this.engines.customMatrix(lon, lat, altitude),
      () => this.getCamera(),
    );

    return this.place(custom, options.before);
  }

  // ── Frames ──

  /** Asks for a frame; several calls before the next frame draw it once */
  requestRender(): void {
    this.frames.request();
  }

  /** @internal Runs `step` every frame until it returns false; replaces the running animation */
  animate(step: Step | undefined): void {
    this.frames.animate(step);
  }

  /** @internal The pointer moved over the map (undefined: it left). Without 'hover' and 'hoverfeature' listeners,
   * nothing is drawn: a still map stays idle under a moving mouse */
  hover(point: Point | undefined): void {
    if (!point) {
      this.frames.hover(undefined);
      this.hoverPick.cancel();

      return;
    }
    if (!this.listeners.has('hover') && !this.listeners.has('hoverfeature')) return;
    this.frames.hover(point);
    this.requestRender();
  }

  private resize(): void {
    if (this.lost) return;

    const { clientWidth: width, clientHeight: height } = this.canvas;
    const ratio = window.devicePixelRatio || 1;

    this.canvas.width = Math.round(width * ratio);
    this.canvas.height = Math.round(height * ratio);
    this.engine.resize(width, height, ratio);
    this.overlays.size = [width, height];
    this.emit('resize', { width, height });
    // Draw at once: a resized canvas is cleared, waiting one frame would flash
    this.frames.drawNow();
  }

  destroy(): void {
    if (this.gone) return;
    this.gone = true;
    // A context restored later rebuilds nothing
    this.unwatchContext();

    const camera = this.getCamera();

    this.frames.stop();
    this.hoverPick.cancel();
    this.basemaps.destroy();
    [...this.vectorSets].forEach((tiles) => {
      tiles.destroy();
    });
    this.vectorSets.clear();
    this.workers.terminate();
    this.unwatchRatio();
    this.unwatchHeaders();
    this.resizeObserver.disconnect();
    this.interaction?.destroy();
    this.engines.destroy();
    // Custom layers free their WebGL objects, models their data, while the context is there
    this.stack.dispose();
    this.engine.free();
    // Work still under way (labels being shaped, tiles arriving, timers) may call the engine after this: it finds one
    // that does nothing, and nothing is drawn again
    this.#engine = destroyedEngine(camera);
    // Glyph ranges still arriving upload to it, not to the freed engine
    this.atlases.attach(this.#engine);
    this.lost = true;
    this.overlays.destroy();
    this.listeners.clear();
  }
}
