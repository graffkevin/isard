/**
 * Handles of the layers added to a map. Each one keeps what it needs to be created again: after a WebGL context loss,
 * the map recreates its engine and every layer in order, with the same handle (its id is updated). Also the page's
 * own 3D content: custom layers (its WebGL code called in every frame) and glTF models drawn at positions.
 */
import { type Feature, parseColor, rampTexels, stops } from './features.ts';
import { mercatorX, mercatorY, toMercator } from './geo.ts';
import type { GeoMap } from './map.ts';
import { rgb } from './settings.ts';
import type {
  LayerHost,
  LonLat,
  NetworkOptions,
  NetworkStyle,
  RasterOptions,
  RasterStyle,
  Tiles3dStyle,
} from './types.ts';
import type { Camera, Engine, NetworkStats, Tiles3dStats } from './wasm/isard.js';

export abstract class Layer {
  #id = -1;

  /** @internal */
  protected readonly map: LayerHost;

  /** @internal */
  constructor(map: LayerHost) {
    this.map = map;
  }

  /** Id of the layer in the engine */
  get id(): number {
    return this.#id;
  }

  #ready: Promise<void> | undefined;

  /** Resolves once the map has drawn the layer, what it shows in view loaded (the map idle after it was added; a
   * layer waiting for the engine with 3D Tiles, once that one has drawn it) */
  get ready(): Promise<void> {
    this.#ready ??= (async () => {
      do await this.map.whenIdle();
      while (this.#id < 0 && this.needsFullEngine);
    })();
    return this.#ready;
  }

  /** @internal Creates the layer in an engine (again after a context loss); its id is -1 if that fails */
  attach(engine: Engine): void {
    this.#id = -1;
    this.#id = this.create(engine);
  }

  /** @internal Forgets its id in an engine that is gone (before the layers are created in a new one) */
  forgetEngine(): void {
    this.#id = -1;
  }

  /** @internal Sends what refers to other layers, once every layer of a new engine has its id */
  relink(_engine: Engine): void {}

  /** @internal Drawn by the engine with 3D Tiles only (3D Tiles, models): on the core engine, it waits for that one */
  get needsFullEngine(): boolean {
    return false;
  }

  /** @internal The layer is removed or its map destroyed: frees what it holds outside the engine */
  dispose(): void {}

  /** @internal */
  protected abstract create(engine: Engine): number;

  remove(): void {
    // Not created yet: a 3D Tiles layer or a model waiting for the engine with 3D Tiles
    if (this.#id >= 0) this.map.engine.removeLayer(this.#id);
    this.#id = -1;
    this.map.detach(this);
    this.map.requestRender();
  }

  /** Moves the layer under `other`, or on top of every layer (above the labels of a vector basemap too) */
  moveBefore(other: Layer | undefined): void {
    this.map.moveLayer(this, other);
  }
}

/** Raster tiles (basemaps, overlays) */
export class RasterLayer extends Layer {
  /** @internal */
  constructor(
    map: LayerHost,
    private readonly options: Required<Omit<RasterOptions, 'size' | 'loader' | 'grid' | 'attribution'>> & {
      size: number | undefined;
      loader: RasterOptions['loader'] | undefined;
    },
  ) {
    super(map);
  }

  /** Changes the opacity or saturation, the other kept */
  setStyle(style: Partial<RasterStyle>): void {
    this.options.opacity = style.opacity ?? this.options.opacity;
    this.options.saturation = style.saturation ?? this.options.saturation;
    this.map.engine.setRasterStyle(this.id, this.options.opacity, this.options.saturation);
    this.map.requestRender();
  }

  /** @internal */
  protected create(engine: Engine): number {
    const { url, minZoom, maxZoom, opacity, saturation, size, loader } = this.options;
    return engine.addRaster(url, minZoom, maxZoom, opacity, saturation, size, loader);
  }
}

/** Line network colored by a value per node (travel time, water depth…) */
export class NetworkLayer extends Layer {
  private values: Float32Array | undefined;
  /** Node and line positions in Mercator, kept for a context loss: f32 offsets from an origin (exact to the centimetre
   * across a region, half the memory of f64) */
  private kept: { origin: [number, number]; nodes: Float32Array; lines: Float32Array | undefined } | undefined;

  /** @internal */
  constructor(
    map: LayerHost,
    private geometry: { nodes: Float64Array; edges: Uint32Array; lines: NetworkOptions['lines'] | undefined },
    private costs: Float32Array | undefined,
    private style: NetworkStyle,
    private edgeWidths: Float32Array | undefined,
  ) {
    super(map);
  }

  /** One value per node, in node order; negative (or 1e30 and above) = unreached. Costs one texture upload, drawn
   * next frame. */
  setValues(values: Float32Array): void {
    this.values = values;
    this.map.engine.setNetworkValues(this.id, values);
    this.map.requestRender();
  }

  /** Costs of the edges, A → B then B → A (see `NetworkOptions.costs`), when the way the network is travelled
   * changes; undefined: values interpolated along the edges. The geometry stays on the graphics card. */
  setCosts(costs: Float32Array | undefined): void {
    this.costs = costs;
    this.map.engine.setNetworkCosts(this.id, costs);
    this.map.requestRender();
  }

  setStyle(style: Partial<NetworkStyle>): void {
    const ramp = this.style.ramp;
    this.style = { ...this.style, ...style };
    this.map.engine.setNetworkStyle(this.id, networkStyle(this.style));
    if (this.style.ramp !== ramp)
      this.map.engine.setNetworkRamp(this.id, this.style.ramp && rampTexels(this.style.ramp).rgba);
    this.map.requestRender();
  }

  /** Width factor of each edge (undefined: 1 for all) */
  setEdgeWidths(widths: Float32Array | undefined): void {
    this.edgeWidths = widths;
    this.map.engine.setNetworkScales(this.id, widths);
    this.map.requestRender();
  }

  /** Pieces in all and drawn in the last frame */
  stats(): NetworkStats | undefined {
    return this.map.engine.networkStats(this.id);
  }

  /** @internal */
  protected create(engine: Engine): number {
    const { nodes, edges, lines } = this.geometry;
    let mercator: { nodes: Float64Array; lines: Float64Array | undefined };
    if (this.kept) {
      const { origin, nodes: n, lines: l } = this.kept;
      mercator = { nodes: unpack(n, origin), lines: l && unpack(l, origin) };
    } else {
      // Mercator here: Math.tan and Math.log run natively, several times faster than in WebAssembly
      mercator = { nodes: mercatorPoints(nodes), lines: lines && mercatorPoints(lines.points) };
      const origin: [number, number] = [mercator.nodes[0] ?? 0, mercator.nodes[1] ?? 0];
      this.kept = {
        origin,
        nodes: pack(mercator.nodes, origin),
        lines: mercator.lines && pack(mercator.lines, origin),
      };
      // The degrees are not needed again: millions of points for a region
      this.geometry = { nodes: EMPTY, edges, lines: lines && { start: lines.start, points: EMPTY } };
    }
    const id = engine.addNetwork(
      mercator.nodes,
      edges,
      lines?.start,
      mercator.lines,
      this.costs,
      networkStyle(this.style),
    );
    if (this.style.ramp) engine.setNetworkRamp(id, rampTexels(this.style.ramp).rgba);
    if (this.edgeWidths) engine.setNetworkScales(id, this.edgeWidths);
    if (this.values) engine.setNetworkValues(id, this.values);
    return id;
  }
}

/** The 25 numbers of a network style for the engine */
function networkStyle(style: NetworkStyle): Float32Array {
  const { width, halo, max, contour, opacity, haloOpacity, unreachedColor, fade } = style;
  return Float32Array.from([
    ...stops(width),
    ...stops(halo),
    max,
    contour,
    opacity,
    haloOpacity,
    ...parseColor(unreachedColor),
    fade,
  ]);
}

const EMPTY = new Float64Array(0);

/** Interleaved points as f32 offsets from an origin, and back */
function pack(points: Float64Array, [ox, oy]: [number, number]): Float32Array {
  const out = new Float32Array(points.length);
  for (let i = 0; i < points.length; i += 2) {
    out[i] = (points[i] ?? 0) - ox;
    out[i + 1] = (points[i + 1] ?? 0) - oy;
  }
  return out;
}

function unpack(points: Float32Array, [ox, oy]: [number, number]): Float64Array {
  const out = new Float64Array(points.length);
  for (let i = 0; i < points.length; i += 2) {
    out[i] = (points[i] ?? 0) + ox;
    out[i + 1] = (points[i + 1] ?? 0) + oy;
  }
  return out;
}

/** Longitudes and latitudes (degrees, interleaved) to Mercator [0, 1], y towards the south */
function mercatorPoints(lonLat: Float64Array): Float64Array {
  const out = new Float64Array(lonLat.length);
  for (let i = 0; i < lonLat.length; i += 2) {
    out[i] = mercatorX(lonLat[i] ?? 0);
    out[i + 1] = mercatorY(lonLat[i + 1] ?? 0);
  }
  return out;
}

/** Textured meshes in 3D Tiles */
export class Tiles3dLayer extends Layer {
  /** @internal */
  constructor(
    map: LayerHost,
    private readonly url: string,
    private readonly geoid: boolean,
    private style: Tiles3dStyle,
  ) {
    super(map);
  }

  setStyle(style: Partial<Tiles3dStyle>): void {
    this.style = { ...this.style, ...style };
    // Kept for its creation while the engine with 3D Tiles loads
    if (this.id < 0) return;
    this.applyStyle(this.map.engine, this.id);
    this.map.requestRender();
  }

  /** @internal */
  override get needsFullEngine(): boolean {
    return true;
  }

  /** @internal The feature picked (`pickFeature`, 'hoverfeature'): the properties of its feature in the tile
   * (`EXT_structural_metadata`, a b3dm batch table), none for a mesh without features; its index in the tile as id */
  featureAt(index: number): Feature<Record<string, unknown>> | undefined {
    if (this.id < 0) return undefined;
    const hit = this.map.engine.tiles3dFeature(this.id, index);
    if (!hit) return undefined;
    const [properties, feature] = hit;
    return { type: 'Feature', id: feature, properties: properties ?? {}, geometry: null };
  }

  /** Loading counters; undefined once the layer is removed, and while the engine with 3D Tiles loads (see
   * `GeoMapOptions.tiles3d`) */
  stats(): Tiles3dStats | undefined {
    return this.id < 0 ? undefined : this.map.engine.tiles3dStats(this.id);
  }

  /** @internal */
  protected create(engine: Engine): number {
    const id = engine.addTiles3d(this.url, this.style.maxScreenSpaceError, this.geoid);
    this.applyStyle(engine, id);
    return id;
  }

  private applyStyle(engine: Engine, id: number): void {
    const { maxScreenSpaceError, opacity, pointSize, color } = this.style;
    engine.setTiles3dStyle(id, maxScreenSpaceError, opacity, pointSize, new Float32Array(color ? rgb(color) : []));
  }
}

/** A texture of a ground layer (see `GroundLayer`) */
export interface GroundTexture {
  /** `r8`, `r8ui`, `r16ui`, `r16i`, `r32ui`, `r32f`, `rg16ui`, `rg32f`, `rgba8`, `rgba8ui`, `rgba16ui`, `rgba32f`:
   * integer formats are read with `usampler2D` / `isampler2D` and `texelFetch` */
  format: string;
  width: number;
  height: number;
  /** Layers of a 2D array (`sampler2DArray`, `usampler2DArray`…); 0 or undefined: a 2D texture */
  depth?: number;
  /** Linear filtering (non-integer formats); default nearest */
  linear?: boolean;
  /** Texels row after row (layer after layer), any typed array; undefined: left empty */
  data?: ArrayBufferView;
}

export interface GroundOptions {
  /**
   * GLSL ES 3.0 defining `vec4 ground(vec2 offset)`: the color of the ground (premultiplied alpha, 0 for nothing) at
   * `offset`, the Mercator offset of the pixel from `anchor` (times `ISARD_CIRCUMFERENCE`: metres of EPSG:3857), exact to
   * the millimetre at any zoom. The code declares its own uniforms and samplers; also there: `u_zoom`,
   * `u_pixel_ratio`
   */
  code: string;
  /** Where the offsets are taken from */
  anchor: LonLat;
  /** Box it shows in, `[west, south, east, north]` (default the world): only the ground there is drawn again */
  bounds?: [west: number, south: number, east: number, north: number];
  textures?: Record<string, GroundTexture>;
  uniforms?: Record<string, number | readonly number[]>;
}

const bytesOf = (data: ArrayBufferView) => new Uint8Array(data.buffer, data.byteOffset, data.byteLength);

/** The application's fragment shader run over the ground, draped on the relief (water depth, heat maps…) */
export class GroundLayer extends Layer {
  private readonly textures = new Map<string, GroundTexture>();
  private readonly uniforms = new Map<string, number[]>();
  private visible = true;

  /** @internal */
  constructor(
    map: LayerHost,
    private readonly options: GroundOptions,
  ) {
    super(map);
    for (const [name, texture] of Object.entries(options.textures ?? {})) this.textures.set(name, texture);
    for (const [name, value] of Object.entries(options.uniforms ?? {})) this.uniforms.set(name, [value].flat());
  }

  /** Creates or replaces a texture of the code's sampler `name` */
  setTexture(name: string, texture: GroundTexture): void {
    this.textures.set(name, texture);
    this.sendTexture(this.map.engine, this.id, name, texture);
    this.map.requestRender();
  }

  /**
   * Replaces a region of a texture: `data` holds `width` × `height` texels for (`x`, `y`) of `layer` (2D arrays).
   * Keep the texture's own `data` up to date too if it must survive a WebGL context loss.
   */
  updateTexture(
    name: string,
    data: ArrayBufferView,
    region: { x?: number; y?: number; layer?: number; width: number; height: number },
  ): void {
    const { x = 0, y = 0, layer = 0, width, height } = region;
    this.map.engine.updateGroundTexture(this.id, name, x, y, layer, width, height, bytesOf(data));
    this.map.requestRender();
  }

  /** A uniform of the code: a number, or numbers for vectors, arrays and matrices (column major) */
  setUniform(name: string, value: number | readonly number[]): void {
    const values = [value].flat();
    this.uniforms.set(name, values);
    this.map.engine.setGroundUniform(this.id, name, Float64Array.from(values));
    this.map.requestRender();
  }

  setBounds(bounds: [west: number, south: number, east: number, north: number]): void {
    this.options.bounds = bounds;
    this.map.engine.setGroundBounds(this.id, ...mercatorBox(bounds));
    this.map.requestRender();
  }

  setVisible(visible: boolean): void {
    this.visible = visible;
    this.map.engine.setGroundVisible(this.id, visible);
    this.map.requestRender();
  }

  private sendTexture(engine: Engine, id: number, name: string, t: GroundTexture): void {
    const data = t.data && bytesOf(t.data);
    engine.setGroundTexture(id, name, t.format, t.width, t.height, t.depth ?? 0, t.linear ?? false, data);
  }

  /** @internal */
  protected create(engine: Engine): number {
    const [ax, ay] = toMercator(this.options.anchor);
    const id = engine.addGround(
      this.options.code,
      ax,
      ay,
      ...mercatorBox(this.options.bounds ?? [-180, -85.06, 180, 85.06]),
    );
    for (const [name, texture] of this.textures) this.sendTexture(engine, id, name, texture);
    for (const [name, values] of this.uniforms) engine.setGroundUniform(id, name, Float64Array.from(values));
    engine.setGroundVisible(id, this.visible);
    return id;
  }
}

/** A lon/lat box to Mercator min x, min y, max x, max y (y towards the south) */
function mercatorBox([west, south, east, north]: [number, number, number, number]): [number, number, number, number] {
  const [x0, y1] = toMercator([west, south]);
  const [x1, y0] = toMercator([east, north]);
  return [x0, y0, x1, y1];
}

// ── The page's own 3D content: custom layers, glTF models ──

/** Reports an error of the page without throwing it here: the console shows it, as an uncaught one */
function report(error: unknown): void {
  if (typeof reportError === 'function') reportError(error);
  else
    queueMicrotask(() => {
      throw error;
    });
}

/** What a custom layer gets to draw a frame (`CustomLayerInterface.render`) */
export interface CustomFrame {
  /**
   * The model-view-projection matrix of a local frame at a position and altitude (metres above the ellipsoid, as the
   * relief is drawn: times its exaggeration), column-major for `uniformMatrix4fv` or a three.js camera: x east, y
   * north, z up, in metres. Made in double precision relative to the camera, so that what is drawn stays exact at
   * any zoom; draw within a few kilometres of the position (or take one matrix per object far apart). Only during
   * `render`.
   */
  matrixAt(lonLat: LonLat, altitude?: number): Float32Array;
  /** The camera of the frame */
  readonly camera: Camera;
}

/**
 * A layer drawn by the page with its own WebGL code, as MapLibre's `CustomLayerInterface`: `render` is called at the
 * layer's place in every frame drawn. It starts with the canvas bound, its viewport set, blending on (`SRC_ALPHA`,
 * `ONE_MINUS_SRC_ALPHA`), no culling, and in `'3d'` mode the depth test on (`LEQUAL`, depth written): the relief, the
 * buildings and the 3D Tiles hide what it draws and what it draws hides them. Whatever WebGL state it changes is set
 * back afterwards; a library that caches the WebGL state (three.js) must forget it first (`renderer.resetState()`).
 *
 * The map is busy drawing meanwhile: `render` reads the frame, not the map (`getCamera` excepted); it may call
 * `requestRender` for an animation.
 */
export interface CustomLayerInterface {
  /** A name for the application (the map numbers its layers) */
  id?: string;
  /** `'2d'` (the default): drawn over what is under it; `'3d'`: with the depth buffer of the 3D layers */
  renderingMode?: '2d' | '3d';
  /** Creates the WebGL objects: once added, and again after the WebGL context was lost and restored */
  onAdd?(gl: WebGL2RenderingContext, map: GeoMap): void;
  render(gl: WebGL2RenderingContext, frame: CustomFrame): void;
  /** Frees the WebGL objects: the layer is removed, or the map destroyed */
  onRemove?(gl: WebGL2RenderingContext, map: GeoMap): void;
}

/** The model-view-projection of a local frame in the engine running the map (`Engine.customMatrix`) */
/** @internal */
export type CustomMatrix = (lon: number, lat: number, altitude: number) => Float32Array | undefined;

/** A layer drawn by the page's WebGL code (see `CustomLayerInterface`) */
export class CustomLayer extends Layer {
  /** The map's WebGL context, and whether `onAdd` made the layer's objects there (until `onRemove`) */
  private readonly gl: WebGL2RenderingContext;
  private added = false;
  /** The context was lost since `onAdd`: its objects are gone */
  private stale = false;
  private readonly lost = () => {
    this.stale = true;
  };
  private readonly frame: CustomFrame;

  /** @internal */
  constructor(
    map: LayerHost,
    /** The application's object */
    readonly implementation: CustomLayerInterface,
    private readonly canvas: HTMLCanvasElement,
    private readonly owner: GeoMap,
    matrix: CustomMatrix,
    camera: () => Camera,
  ) {
    super(map);
    // The context the engine made on the canvas
    this.gl = canvas.getContext('webgl2') as WebGL2RenderingContext;
    this.frame = {
      matrixAt: ([lon, lat], altitude = 0) => {
        const m = matrix(lon, lat, altitude);
        if (!m) throw new Error('isard: frame.matrixAt is only available during render');
        return m;
      },
      get camera() {
        return camera();
      },
    };
  }

  /** @internal */
  protected create(engine: Engine): number {
    // Once, and again on a context restored (not when the map moves onto another engine: the same context)
    if (!this.added || this.stale) {
      if (!this.added) this.canvas.addEventListener('webglcontextlost', this.lost);
      this.added = true;
      this.stale = false;
      this.implementation.onAdd?.(this.gl, this.owner);
    }
    return engine.addCustomLayer(this.draw, this.implementation.renderingMode === '3d');
  }

  /** Called by the engine at the layer's place in the frame; the page's errors are reported, the frame goes on */
  private readonly draw = (): void => {
    try {
      this.implementation.render(this.gl, this.frame);
    } catch (error) {
      report(error);
    }
  };

  override remove(): void {
    super.remove();
    this.dispose();
  }

  /** @internal The layer is removed or the map destroyed: `onRemove`, unless the context is lost */
  override dispose(): void {
    if (!this.added) return;
    this.added = false;
    this.canvas.removeEventListener('webglcontextlost', this.lost);
    if (this.stale) return;
    try {
      this.implementation.onRemove?.(this.gl, this.owner);
    } catch (error) {
      report(error);
    }
  }
}

/** Where and how a model stands */
export interface ModelInstance {
  lonLat: LonLat;
  /** Metres above the ground (or above the ellipsoid, `altitudeReference: 'sea'`); 0 by default */
  altitude?: number;
  /** Degrees clockwise from the north of the model's front (glTF +Z); 0 by default */
  heading?: number;
  /** Degrees, nose up */
  pitch?: number;
  /** Degrees, right side down */
  roll?: number;
  /** 1 by default: the model's metres */
  scale?: number;
}

export interface ModelOptions {
  /** URL of a glTF binary (`.glb`): meshes, textures, Draco, meshopt, KTX2, as in 3D Tiles; or its bytes (`data`) */
  url?: string;
  data?: ArrayBuffer | ArrayBufferView<ArrayBuffer>;
  instances: readonly ModelInstance[];
  /** What the altitudes are above: `'ground'` (the relief, the default) or `'sea'` (the ellipsoid) */
  altitudeReference?: 'ground' | 'sea';
}

/** The numbers of instances for the engine: longitude, latitude, altitude, heading, pitch, roll, scale */
export function instanceNumbers(instances: readonly ModelInstance[]): Float64Array {
  const out = new Float64Array(instances.length * 7);
  instances.forEach(({ lonLat, altitude = 0, heading = 0, pitch = 0, roll = 0, scale = 1 }, i) => {
    out.set([lonLat[0], lonLat[1], altitude, heading, pitch, roll, scale], i * 7);
  });
  return out;
}

/** A glTF model drawn at positions, in one instanced draw; `pickFeature` gives the instance's index */
export class ModelLayer extends Layer {
  private readonly url: string;
  /** The `blob:` URL of the page's bytes, revoked once the layer is gone */
  private readonly blob: string | undefined;
  private instances: Float64Array;
  private ground: boolean;

  /** @internal */
  constructor(map: LayerHost, options: ModelOptions) {
    super(map);
    const { url, data } = options;
    if (data) {
      this.blob = URL.createObjectURL(new Blob([data], { type: 'model/gltf-binary' }));
      this.url = this.blob;
    } else if (url) {
      // Against the page: the decoding workers fetch it, and a relative URL would be theirs
      const base = typeof document === 'undefined' ? undefined : document.baseURI;
      this.url = base ? new URL(url, base).href : url;
    } else {
      throw new Error('isard: a model needs a url or data');
    }
    this.instances = instanceNumbers(options.instances);
    this.ground = options.altitudeReference !== 'sea';
  }

  /** @internal The engine with 3D Tiles decodes and draws the models */
  override get needsFullEngine(): boolean {
    return true;
  }

  /** Replaces the instances (and what their altitudes are above, kept if not given) */
  setInstances(instances: readonly ModelInstance[], altitudeReference?: 'ground' | 'sea'): void {
    this.instances = instanceNumbers(instances);
    if (altitudeReference) this.ground = altitudeReference === 'ground';
    // Kept for its creation while the engine with 3D Tiles loads
    if (this.id < 0) return;
    this.map.engine.setModelInstances(this.id, this.instances, this.ground);
    this.map.requestRender();
  }

  override remove(): void {
    super.remove();
    this.dispose();
  }

  /** @internal */
  override dispose(): void {
    if (this.blob) URL.revokeObjectURL(this.blob);
  }

  /** @internal */
  protected create(engine: Engine): number {
    return engine.addModels(this.url, this.instances, this.ground);
  }
}
