/** Handles of the layers added to a map */

import type { LayerHost, NetworkStyle, Tiles3dStyle } from './types.ts';
import type { Tiles3dStats } from './wasm/geo_render.js';

abstract class Layer {
  constructor(
    protected readonly map: LayerHost,
    readonly id: number,
  ) {}

  remove(): void {
    this.map.engine.removeLayer(this.id);
    this.map.requestRender();
  }
}

/** Raster tiles (basemaps, overlays) */
export class RasterLayer extends Layer {
  setStyle(opacity: number, saturation = 1): void {
    this.map.engine.setRasterStyle(this.id, opacity, saturation);
    this.map.requestRender();
  }
}

/** Line network colored by a value per node (travel time, water depth…) */
export class NetworkLayer extends Layer {
  constructor(
    map: LayerHost,
    id: number,
    private style: NetworkStyle,
  ) {
    super(map, id);
  }

  /** One value per node, in node order; negative = unreached. Costs one texture upload, drawn next frame. */
  setValues(values: Float32Array): void {
    this.map.engine.setNetworkValues(this.id, values);
    this.map.requestRender();
  }

  setStyle(style: Partial<NetworkStyle>): void {
    this.style = { ...this.style, ...style };
    const { width, max, contour, opacity } = this.style;
    this.map.engine.setNetworkStyle(this.id, width, max, contour, opacity);
    this.map.requestRender();
  }
}

/** Textured meshes in 3D Tiles */
export class Tiles3dLayer extends Layer {
  constructor(
    map: LayerHost,
    id: number,
    private style: Tiles3dStyle,
  ) {
    super(map, id);
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
}
