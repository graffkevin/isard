/**
 * Handles of the layers added to a map. Each one keeps what it needs to be created again: after a WebGL context loss,
 * the map recreates its engine and every layer in order, with the same handle (its id is updated).
 */
import type { LayerHost, NetworkOptions, NetworkStyle, RasterOptions, Tiles3dStyle } from './types.ts';
import type { Engine, NetworkStats, Tiles3dStats } from './wasm/geo_render.js';

export abstract class Layer {
  #id = -1;

  constructor(protected readonly map: LayerHost) {}

  /** Id of the layer in the engine */
  get id(): number {
    return this.#id;
  }

  /** @internal Creates the layer in an engine (again after a context loss) */
  attach(engine: Engine): void {
    this.#id = this.create(engine);
  }

  protected abstract create(engine: Engine): number;

  remove(): void {
    this.map.engine.removeLayer(this.#id);
    this.map.detach(this);
    this.map.requestRender();
  }
}

/** Raster tiles (basemaps, overlays) */
export class RasterLayer extends Layer {
  constructor(
    map: LayerHost,
    private readonly options: Required<RasterOptions>,
  ) {
    super(map);
  }

  setStyle(opacity: number, saturation = 1): void {
    this.options.opacity = opacity;
    this.options.saturation = saturation;
    this.map.engine.setRasterStyle(this.id, opacity, saturation);
    this.map.requestRender();
  }

  protected create(engine: Engine): number {
    const { url, minZoom, maxZoom, opacity, saturation } = this.options;
    return engine.addRaster(url, minZoom, maxZoom, opacity, saturation);
  }
}

/** Line network colored by a value per node (travel time, water depth…) */
export class NetworkLayer extends Layer {
  private values: Float32Array | undefined;
  /** Node and line positions in Mercator, kept for a context loss */
  private mercator: { nodes: Float64Array; lines: Float64Array | undefined } | undefined;

  constructor(
    map: LayerHost,
    private readonly geometry: { nodes: Float64Array; edges: Uint32Array; lines: NetworkOptions['lines'] | undefined },
    private costs: Float32Array | undefined,
    private style: NetworkStyle,
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
    this.style = { ...this.style, ...style };
    const { width, max, contour, opacity, halo, haloOpacity } = this.style;
    this.map.engine.setNetworkStyle(this.id, width, max, contour, opacity, halo, haloOpacity);
    this.map.requestRender();
  }

  /** Pieces in all and drawn in the last frame */
  stats(): NetworkStats | undefined {
    return this.map.engine.networkStats(this.id);
  }

  protected create(engine: Engine): number {
    const { nodes, edges, lines } = this.geometry;
    const { width, max, contour, opacity, halo, haloOpacity } = this.style;
    // Mercator here: Math.tan and Math.log run natively, several times faster than in WebAssembly
    this.mercator ??= { nodes: toMercator(nodes), lines: lines && toMercator(lines.points) };
    const id = engine.addNetwork(
      this.mercator.nodes,
      edges,
      lines?.start,
      this.mercator.lines,
      this.costs,
      width,
      max,
      contour,
      opacity,
      halo,
      haloOpacity,
    );
    if (this.values) engine.setNetworkValues(id, this.values);
    return id;
  }
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

  protected create(engine: Engine): number {
    const id = engine.addTiles3d(this.url, this.style.maxScreenSpaceError);
    engine.setTiles3dStyle(id, this.style.maxScreenSpaceError, this.style.opacity);
    return id;
  }
}
