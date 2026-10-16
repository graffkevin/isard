/**
 * Handles of the layers added to a map. Each one keeps what it needs to be created again: after a WebGL context loss,
 * the map recreates its engine and every layer in order, with the same handle (its id is updated).
 */
import { parseColor, rampTexels, stops } from './features.ts';
import type { LayerHost, LonLat, NetworkOptions, NetworkStyle, RasterOptions, Tiles3dStyle } from './types.ts';
import type { Engine, NetworkStats, Tiles3dStats } from './wasm/isard.js';

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

  /** @internal Creates the layer in an engine (again after a context loss) */
  attach(engine: Engine): void {
    this.#id = this.create(engine);
  }

  /** @internal */
  protected abstract create(engine: Engine): number;

  remove(): void {
    this.map.engine.removeLayer(this.#id);
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
    private readonly options: Required<Omit<RasterOptions, 'size' | 'loader'>> & {
      size: number | undefined;
      loader: RasterOptions['loader'] | undefined;
    },
  ) {
    super(map);
  }

  setStyle(opacity: number, saturation = 1): void {
    this.options.opacity = opacity;
    this.options.saturation = saturation;
    this.map.engine.setRasterStyle(this.id, opacity, saturation);
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
      mercator = { nodes: toMercator(nodes), lines: lines && toMercator(lines.points) };
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

const MAX_LAT = 85.051128779806589;

/** Longitudes and latitudes (degrees, interleaved) to Mercator [0, 1], y towards the south */
function toMercator(lonLat: Float64Array): Float64Array {
  const out = new Float64Array(lonLat.length);
  for (let i = 0; i < lonLat.length; i += 2) {
    const lat = (Math.max(-MAX_LAT, Math.min(MAX_LAT, lonLat[i + 1] ?? 0)) * Math.PI) / 180;
    out[i] = ((lonLat[i] ?? 0) + 180) / 360;
    out[i + 1] = 0.5 - Math.log(Math.tan(Math.PI / 4 + lat / 2)) / (2 * Math.PI);
  }
  return out;
}

/** Textured meshes in 3D Tiles */
export class Tiles3dLayer extends Layer {
  /** @internal */
  constructor(
    map: LayerHost,
    private readonly url: string,
    private style: Tiles3dStyle,
  ) {
    super(map);
  }

  setStyle(style: Partial<Tiles3dStyle>): void {
    this.style = { ...this.style, ...style };
    this.map.engine.setTiles3dStyle(this.id, this.style.maxScreenSpaceError, this.style.opacity);
    this.map.requestRender();
  }

  /** Loading counters; undefined once the layer is removed */
  stats(): Tiles3dStats | undefined {
    return this.map.engine.tiles3dStats(this.id);
  }

  /** @internal */
  protected create(engine: Engine): number {
    const id = engine.addTiles3d(this.url, this.style.maxScreenSpaceError);
    engine.setTiles3dStyle(id, this.style.maxScreenSpaceError, this.style.opacity);
    return id;
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
   * `offset`, the Mercator offset of the pixel from `anchor` (times `ISARD_EARTH`: metres of EPSG:3857), exact to
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
    const [ax, ay] = mercatorPoint(this.options.anchor);
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

function mercatorPoint([lon, lat]: LonLat): [number, number] {
  const out = toMercator(Float64Array.of(lon, lat));
  return [out[0] ?? 0, out[1] ?? 0];
}

/** A lon/lat box to Mercator min x, min y, max x, max y (y towards the south) */
function mercatorBox([west, south, east, north]: [number, number, number, number]): [number, number, number, number] {
  const [x0, y1] = mercatorPoint([west, south]);
  const [x1, y0] = mercatorPoint([east, north]);
  return [x0, y0, x1, y1];
}
