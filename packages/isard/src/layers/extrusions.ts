/**
 * Extruded polygons (buildings), styled per feature like the other feature layers, coming in tiles (or one set): an
 * application loading a tiled source (BD TOPO vector tiles…) hands each tile's buildings to `setTile` and drops them
 * with `removeTile`. Colored by their color, by a value per building through the ramp (water at a building), or by
 * the node values of a network layer (`linkNetwork`: the travel time to each building, recolored with the network).
 */

import type { LayerHost } from '../types.ts';
import type { Engine } from '../wasm/isard.js';
import {
  evaluate,
  type Feature,
  type FeatureData,
  type FeatureLayerStyle,
  FLOATS,
  featuresOf,
  type Placement,
  type Properties,
  parseColor,
  polygonGeometry,
  rampTexels,
  type Styled,
  stops,
} from './features.ts';
import { Layer, type NetworkLayer } from './layers.ts';

export interface ExtrusionStyle<P = Properties> extends FeatureLayerStyle<P> {
  /** CSS color (default `#e4e1dc`) */
  color: Styled<string, P>;
  /** Height of the roof over the ground, metres (default 6) */
  height: Styled<number, P>;
  /** Height of the foot of the walls, metres (default 0) */
  base: Styled<number, P>;
}

export interface ExtrusionOptions<P = Properties> extends Partial<ExtrusionStyle<P>>, Placement {
  /** Polygon and MultiPolygon features, as one set (tiles come with `setTile`) */
  data?: FeatureData<P>;
}

/** Key of a tile of extrusions (see `ExtrusionLayer.setTile`) */
export type TileKey = string | number;

/**
 * Polygons as typed arrays, for tiles decoded in a worker (no GeoJSON in between): points in Mercator [0, 1] (x, y
 * interleaved, y towards the south), the first point of each ring and the first ring of each polygon (one more entry
 * than rings and polygons; the first ring of a polygon is its outline), the feature of each polygon
 */
export interface PolygonArrays {
  points: Float64Array;
  ringStart: Uint32Array;
  polygonStart: Uint32Array;
  feature: Uint32Array;
}

interface Tile<P> {
  /** Batch number in the engine: tiles are drawn, and their features numbered for picking, in this order */
  batch: number;
  /** The properties of the features, which the style reads */
  properties: P[];
  features: Feature<P>[] | undefined;
  arrays: PolygonArrays | undefined;
  values: Float32Array | undefined;
  anchors: Float32Array | undefined;
}

/** TileKey of the set given as `data` */
const DATA = Symbol('data');

export class ExtrusionLayer<P = Properties> extends Layer {
  private readonly tiles = new Map<TileKey | typeof DATA, Tile<P>>();
  private nextBatch = 1;
  private network: { layer: NetworkLayer; max: number } | undefined;

  /** @internal */
  constructor(
    map: LayerHost,
    data: FeatureData<P> | undefined,
    private style: ExtrusionStyle<P>,
  ) {
    super(map);
    if (data) {
      const features = featuresOf(data);
      const properties = features.map((f) => f.properties);
      this.tiles.set(DATA, {
        batch: 0,
        properties,
        features,
        arrays: undefined,
        values: undefined,
        anchors: undefined,
      });
    }
  }

  /** Replaces the set given as `data` */
  setData(data: FeatureData<P>): void {
    const features = featuresOf(data);
    this.put(
      DATA,
      features.map((f) => f.properties),
      features,
      undefined,
    );
  }

  /** Adds or replaces the buildings of a tile: GeoJSON, or typed arrays with the properties of each feature */
  setTile(key: TileKey, data: FeatureData<P> | { arrays: PolygonArrays; properties: P[] }): void {
    if ('arrays' in data) this.put(key, data.properties, undefined, data.arrays);
    else {
      const features = featuresOf(data);
      this.put(
        key,
        features.map((f) => f.properties),
        features,
        undefined,
      );
    }
  }

  removeTile(key: TileKey): void {
    const tile = this.tiles.get(key);
    if (!tile) return;
    this.tiles.delete(key);
    this.map.engine.removeExtrusionBatch(this.id, tile.batch);
    this.map.requestRender();
  }

  /** Keys of the tiles held */
  get tileKeys(): TileKey[] {
    return [...this.tiles.keys()].filter((k): k is TileKey => k !== DATA);
  }

  /** The properties of the features of a tile (or of the data) */
  properties(key?: TileKey): readonly P[] {
    return this.tiles.get(key ?? DATA)?.properties ?? [];
  }

  /** The feature of a picked index (its geometry if given as GeoJSON): tiles are numbered one after the other, in the
   * order they were added */
  featureAt(index: number): Feature<P> | undefined {
    let rest = index;
    for (const tile of [...this.tiles.values()].sort((a, b) => a.batch - b.batch)) {
      const n = tile.properties.length;
      if (rest < n) {
        const properties = tile.properties[rest] as P;
        return tile.features?.[rest] ?? { type: 'Feature', properties, geometry: null };
      }
      rest -= n;
    }
    return undefined;
  }

  /** The tile and the index in it of a picked index */
  tileAt(index: number): { key: TileKey | undefined; index: number } | undefined {
    let rest = index;
    for (const [key, tile] of [...this.tiles].sort((a, b) => a[1].batch - b[1].batch)) {
      if (rest < tile.properties.length) return { key: key === DATA ? undefined : key, index: rest };
      rest -= tile.properties.length;
    }
    return undefined;
  }

  setStyle(style: Partial<ExtrusionStyle<P>>): void {
    this.style = { ...this.style, ...style };
    const engine = this.map.engine;
    this.tiles.forEach((tile) => {
      this.sendStyle(engine, this.id, tile);
    });
    this.sendLayer(engine, this.id);
    this.map.requestRender();
  }

  setVisible(visible: boolean): void {
    this.setStyle({ visible } as Partial<ExtrusionStyle<P>>);
  }

  setFilter(filter: ((properties: P, index: number) => boolean) | undefined): void {
    this.style = { ...this.style, filter };
    this.tiles.forEach((tile) => {
      this.map.engine.setFeatureMask(this.id, this.mask(tile), tile.batch);
    });
    this.map.requestRender();
  }

  /** One value per building of a tile (or of the data), colored through the ramp; NaN keeps a building's color */
  setValues(values: Float32Array | undefined, key?: TileKey): void {
    const tile = this.tiles.get(key ?? DATA);
    if (!tile) return;
    tile.values = values;
    this.map.engine.setFeatureValues(
      this.id,
      values ?? new Float32Array(tile.properties.length).fill(Number.NaN),
      tile.batch,
    );
    this.map.requestRender();
  }

  /**
   * Colors the buildings by the node values of a network layer, recolored with it: each building takes min(value(A)
   * + cost from A, value(B) + cost from B) from its anchors (`setAnchors`), through the ramp, up to `max` (above, or
   * unreached: its own color). Undefined: back to the buildings' own values.
   */
  linkNetwork(network: NetworkLayer | undefined, max = Number.POSITIVE_INFINITY): void {
    this.network = network && { layer: network, max };
    this.sendLink(this.map.engine, this.id);
    this.map.requestRender();
  }

  /** Per building of a tile (or of the data): node A, node B, cost from A, cost from B (4 floats each) */
  setAnchors(anchors: Float32Array, key?: TileKey): void {
    const tile = this.tiles.get(key ?? DATA);
    if (!tile) return;
    tile.anchors = anchors;
    this.map.engine.setExtrusionAnchors(this.id, tile.batch, anchors);
    this.map.requestRender();
  }

  private put(
    key: TileKey | typeof DATA,
    properties: P[],
    features: Feature<P>[] | undefined,
    arrays: PolygonArrays | undefined,
  ): void {
    const old = this.tiles.get(key);
    const tile: Tile<P> = {
      batch: old?.batch ?? (key === DATA ? 0 : this.nextBatch++),
      properties,
      features,
      arrays,
      values: undefined,
      anchors: undefined,
    };
    this.tiles.set(key, tile);
    this.sendTile(this.map.engine, this.id, tile);
    this.map.requestRender();
  }

  private mask(tile: Tile<P>): Uint8Array {
    const { filter } = this.style;
    const mask = new Uint8Array(tile.properties.length).fill(255);
    if (filter)
      tile.properties.forEach((p, i) => {
        if (!filter(p, i)) mask[i] = 0;
      });
    return mask;
  }

  private sendTile(engine: Engine, id: number, tile: Tile<P>): void {
    const g = tile.arrays ?? polygonGeometry(tile.features ?? []);
    const n = tile.properties.length;
    engine.setExtrusionBatch(id, tile.batch, g.points, g.ringStart, g.polygonStart, g.feature, n);
    this.sendStyle(engine, id, tile);
    if (tile.values) engine.setFeatureValues(id, tile.values, tile.batch);
    if (tile.anchors) engine.setExtrusionAnchors(id, tile.batch, tile.anchors);
  }

  private sendStyle(engine: Engine, id: number, tile: Tile<P>): void {
    const { color, height, base } = this.style;
    const out = new Float32Array(tile.properties.length * FLOATS);
    // Constant colors parsed once: a tile has thousands of buildings
    const fixed = typeof color === 'string' ? parseColor(color) : undefined;
    tile.properties.forEach((p, i) => {
      const o = i * FLOATS;
      out.set(fixed ?? parseColor(evaluate(color, p, i)), o);
      out.set(stops(evaluate(height, p, i)), o + 8);
      out.set(stops(evaluate(base, p, i)), o + 16);
    });
    engine.setFeatureStyle(id, out, tile.batch);
    engine.setFeatureMask(id, this.mask(tile), tile.batch);
  }

  private sendLayer(engine: Engine, id: number): void {
    const { ramp, opacity, minZoom, maxZoom, visible } = this.style;
    const r = ramp && rampTexels(ramp);
    engine.setFeatureRamp(id, r?.rgba, r?.min ?? 0, r?.max ?? 1);
    engine.setFeatureLayer(id, Float32Array.from(stops(opacity)), minZoom, maxZoom, visible);
  }

  private sendLink(engine: Engine, id: number): void {
    const n = this.network;
    // A network removed (or not created) links to nothing
    const network = n && n.layer.id >= 0 ? n.layer.id : undefined;
    engine.linkExtrusionNetwork(id, network, n ? Math.min(n.max, 1e29) : 0);
  }

  /** @internal */
  protected create(engine: Engine): number {
    const id = engine.addExtrusions();
    this.sendLayer(engine, id);
    this.tiles.forEach((tile) => {
      this.sendTile(engine, id, tile);
    });
    // A network created before (on a new engine, the map relinks once every layer is there)
    if (!this.network || this.network.layer.id >= 0) this.sendLink(engine, id);
    return id;
  }

  /** @internal On a new engine, the network may come after the buildings: linked once both are there */
  override relink(engine: Engine): void {
    if (this.network) this.sendLink(engine, this.id);
  }
}

export const extrusionStyle = <P>(options: ExtrusionOptions<P>): ExtrusionStyle<P> => {
  const { data: _, before: __, ...rest } = options;
  return {
    opacity: 1,
    minZoom: 0,
    maxZoom: 24,
    visible: true,
    filter: undefined,
    ramp: undefined,
    color: '#e4e1dc',
    height: 6,
    base: 0,
    ...rest,
  };
};
