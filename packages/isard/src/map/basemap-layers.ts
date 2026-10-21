/** The layers of a map's basemap: a raster or vector basemap (its labels kept on top) and the IGN hillshade, under the
 * other layers. */

import { BASEMAPS, HILLSHADE } from '../basemap/basemaps.ts';
import { VECTOR_BASEMAPS, VectorTiles } from '../basemap/vectortiles.ts';
import type { RasterLayer } from '../layers/layers.ts';
import type { SymbolLayer } from '../layers/symbols.ts';
import type { GeoMap } from '../map.ts';
import type { BasemapName } from '../types.ts';
import type { LayerStack } from './layer-stack.ts';

/** The basemap and the hillshade of a map, replaced together */
export class BasemapLayers {
  private readonly map: GeoMap;
  private readonly stack: LayerStack;
  /** URL of the vector basemaps' workers */
  private readonly workerUrl: string | URL;
  /** Texture size of the vector basemap's tiles */
  private readonly tileSize: number | undefined;
  private basemap: RasterLayer | undefined;
  private hillshade: RasterLayer | undefined;
  /** The vector basemap, if any: its tiles' workers and its labels */
  private vector: { tiles: VectorTiles; labels: SymbolLayer<unknown> } | undefined;

  /** `stack`: the map's drawing order */
  constructor(map: GeoMap, stack: LayerStack, workerUrl: string | URL, tileSize: number | undefined) {
    this.map = map;
    this.stack = stack;
    this.workerUrl = workerUrl;
    this.tileSize = tileSize;
  }

  /** Removes the basemap and the hillshade, then adds the basemap `name` (none for 'none') */
  replace(name: BasemapName): void {
    const { map } = this;

    if (this.basemap && map.hasLayer(this.basemap)) this.basemap.remove();
    if (this.hillshade && map.hasLayer(this.hillshade)) this.hillshade.remove();
    // Its raster and labels go with it
    this.destroy();
    this.vector = undefined;

    const vector = VECTOR_BASEMAPS[name as keyof typeof VECTOR_BASEMAPS];

    if (vector) {
      const tiles = new VectorTiles(map, { style: vector, ...(this.workerUrl ? { workerUrl: this.workerUrl } : {}) });

      this.basemap = tiles.raster(this.tileSize ? { size: this.tileSize } : {});
      this.vector = { tiles, labels: tiles.labels() as SymbolLayer<unknown> };
    } else {
      this.basemap = name === 'none' ? undefined : map.addRaster(BASEMAPS[name as keyof typeof BASEMAPS]);
    }
  }

  /** Adds the hillshade at `opacity` (0: none), then puts it and the basemap under all the other layers */
  addHillshade(opacity: number): void {
    this.hillshade = opacity > 0 ? this.map.addRaster({ ...HILLSHADE, opacity }) : undefined;
    if (this.hillshade) this.stack.toBottom(this.hillshade);
    if (this.basemap) this.stack.toBottom(this.basemap);
  }

  /** Stops the vector basemap's tiles (their layers removed) */
  destroy(): void {
    this.vector?.tiles.destroy();
  }
}
