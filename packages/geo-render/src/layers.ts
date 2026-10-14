/**
 * Handles of the layers added to a map. Each one keeps what it needs to be created again: after a WebGL context loss,
 * the map recreates its engine and every layer in order, with the same handle (its id is updated).
 */
import type { LayerHost, NetworkStyle, RasterOptions, Tiles3dStyle } from './types.ts';
import type { Engine, Tiles3dStats } from './wasm/geo_render.js';

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

  constructor(
    map: LayerHost,
    private readonly nodes: Float64Array,
    private readonly edges: Uint32Array,
    private style: NetworkStyle,
  ) {
    super(map);
  }

  /** One value per node, in node order; negative = unreached. Costs one texture upload, drawn next frame. */
  setValues(values: Float32Array): void {
    this.values = values;
    this.map.engine.setNetworkValues(this.id, values);
    this.map.requestRender();
  }

  setStyle(style: Partial<NetworkStyle>): void {
    this.style = { ...this.style, ...style };
    const { width, max, contour, opacity } = this.style;
    this.map.engine.setNetworkStyle(this.id, width, max, contour, opacity);
    this.map.requestRender();
  }

  protected create(engine: Engine): number {
    const { width, max, contour, opacity } = this.style;
    const id = engine.addNetwork(this.nodes, this.edges, width, max, contour, opacity);
    if (this.values) engine.setNetworkValues(id, this.values);
    return id;
  }
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
