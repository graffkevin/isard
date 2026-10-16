/**
 * Extruded polygons (buildings), styled per feature like the other feature layers, coming in tiles (or one set): an
 * application loading a tiled source (BD TOPO vector tiles…) hands each tile's buildings to `setTile` and drops them
 * with `removeTile`. Colored by their color, by a value per building through the ramp (water at a building), or by
 * the node values of a network layer (`linkNetwork`: the travel time to each building, recolored with the network).
 */
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
import type { LayerHost } from './types.ts';
import type { Engine } from './wasm/isard.js';

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

type Key = string | number;

interface Tile<P> {
  /** Batch number in the engine: tiles are drawn, and their features numbered for picking, in this order */
  batch: number;
  features: Feature<P>[];
  values: Float32Array | undefined;
  anchors: Float32Array | undefined;
}

/** Key of the set given as `data` */
const DATA = Symbol('data');

export class ExtrusionLayer<P = Properties> extends Layer {
  private readonly tiles = new Map<Key | typeof DATA, Tile<P>>();
  private nextBatch = 1;
  private network: { layer: NetworkLayer; max: number } | undefined;

  constructor(
    map: LayerHost,
    data: FeatureData<P> | undefined,
    private style: ExtrusionStyle<P>,
  ) {
    super(map);
    if (data) this.tiles.set(DATA, { batch: 0, features: featuresOf(data), values: undefined, anchors: undefined });
  }

  /** Replaces the set given as `data` */
  setData(data: FeatureData<P>): void {
    this.put(DATA, featuresOf(data));
  }

  /** Adds or replaces the buildings of a tile */
  setTile(key: Key, data: FeatureData<P>): void {
    this.put(key, featuresOf(data));
  }

  removeTile(key: Key): void {
    const tile = this.tiles.get(key);
    if (!tile) return;
    this.tiles.delete(key);
    this.map.engine.removeExtrusionBatch(this.id, tile.batch);
    this.map.requestRender();
  }

  /** Keys of the tiles held */
  get tileKeys(): Key[] {
    return [...this.tiles.keys()].filter((k): k is Key => k !== DATA);
  }

  /** The features of a tile (or of the data) */
  features(key?: Key): readonly Feature<P>[] {
    return this.tiles.get(key ?? DATA)?.features ?? [];
  }

  /** The feature of a picked index: tiles are numbered one after the other, in the order they were added */
  featureAt(index: number): Feature<P> | undefined {
    let rest = index;
    for (const tile of [...this.tiles.values()].sort((a, b) => a.batch - b.batch)) {
      if (rest < tile.features.length) return tile.features[rest];
      rest -= tile.features.length;
    }
    return undefined;
  }

  setStyle(style: Partial<ExtrusionStyle<P>>): void {
    this.style = { ...this.style, ...style };
    const engine = this.map.engine;
    for (const tile of this.tiles.values()) this.sendStyle(engine, this.id, tile);
    this.sendLayer(engine, this.id);
    this.map.requestRender();
  }

  setFilter(filter: ((properties: P, index: number) => boolean) | undefined): void {
    this.style = { ...this.style, filter };
    for (const tile of this.tiles.values()) this.map.engine.setFeatureMask(this.id, this.mask(tile), tile.batch);
    this.map.requestRender();
  }

  /** One value per building of a tile (or of the data), colored through the ramp; NaN keeps a building's color */
  setValues(values: Float32Array | undefined, key?: Key): void {
    const tile = this.tiles.get(key ?? DATA);
    if (!tile) return;
    tile.values = values;
    this.map.engine.setFeatureValues(
      this.id,
      values ?? new Float32Array(tile.features.length).fill(Number.NaN),
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
  setAnchors(anchors: Float32Array, key?: Key): void {
    const tile = this.tiles.get(key ?? DATA);
    if (!tile) return;
    tile.anchors = anchors;
    this.map.engine.setExtrusionAnchors(this.id, tile.batch, anchors);
    this.map.requestRender();
  }

  private put(key: Key | typeof DATA, features: Feature<P>[]): void {
    const old = this.tiles.get(key);
    const tile: Tile<P> = {
      batch: old?.batch ?? (key === DATA ? 0 : this.nextBatch++),
      features,
      values: undefined,
      anchors: undefined,
    };
    this.tiles.set(key, tile);
    this.sendTile(this.map.engine, this.id, tile);
    this.map.requestRender();
  }

  private mask(tile: Tile<P>): Uint8Array {
    const { filter } = this.style;
    const mask = new Uint8Array(tile.features.length).fill(255);
    if (filter)
      tile.features.forEach((f, i) => {
        if (!filter(f.properties, i)) mask[i] = 0;
      });
    return mask;
  }

  private sendTile(engine: Engine, id: number, tile: Tile<P>): void {
    const g = polygonGeometry(tile.features);
    engine.setExtrusionBatch(id, tile.batch, g.points, g.ringStart, g.polygonStart, g.feature, tile.features.length);
    this.sendStyle(engine, id, tile);
    if (tile.values) engine.setFeatureValues(id, tile.values, tile.batch);
    if (tile.anchors) engine.setExtrusionAnchors(id, tile.batch, tile.anchors);
  }

  private sendStyle(engine: Engine, id: number, tile: Tile<P>): void {
    const { color, height, base } = this.style;
    const out = new Float32Array(tile.features.length * FLOATS);
    tile.features.forEach((f, i) => {
      const o = i * FLOATS;
      out.set(parseColor(evaluate(color, f.properties, i)), o);
      out.set(stops(evaluate(height, f.properties, i)), o + 8);
      out.set(stops(evaluate(base, f.properties, i)), o + 16);
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
    engine.linkExtrusionNetwork(id, n?.layer.id, n ? Math.min(n.max, 1e29) : 0);
  }

  protected create(engine: Engine): number {
    const id = engine.addExtrusions();
    this.sendLayer(engine, id);
    for (const tile of this.tiles.values()) this.sendTile(engine, id, tile);
    this.sendLink(engine, id);
    return id;
  }
}

export function extrusionStyle<P>(options: ExtrusionOptions<P>): ExtrusionStyle<P> {
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
}
