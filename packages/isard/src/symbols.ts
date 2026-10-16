/**
 * Text labels from GeoJSON: at points, at the middle of lines, at the center of polygons. Shaped on the page, placed by
 * the engine every frame with collisions shared by all symbol layers (the top layer first, each layer in data order),
 * drawn with a halo, standing on the relief.
 */
import {
  evaluate,
  type Feature,
  type FeatureData,
  type FeatureLayerStyle,
  featuresOf,
  type Geometry,
  type Placement,
  type Position,
  type Properties,
  parseColor,
  type Size,
  type Styled,
  stops,
} from './features.ts';
import { codes, type Glyph, type Glyphs, shape, type TextPart } from './glyphs.ts';
import { Layer } from './layers.ts';
import type { Anchor } from './overlay.ts';
import type { LayerHost } from './types.ts';
import type { Engine } from './wasm/isard.js';

export interface SymbolStyle<P = Properties> extends Omit<FeatureLayerStyle<P>, 'ramp'> {
  /** The text: a string (`\n` breaks lines) or runs with their own scale and font; empty: no label */
  text: Styled<string | TextPart[], P>;
  /** Font of the Géoplateforme (default `Source Sans Pro Regular`; also Bold, Italic, Semibold) */
  font: string;
  /** CSS pixels, or zoom stops (default 12) */
  size: Size;
  color: Styled<string, P>;
  haloColor: Styled<string, P>;
  /** CSS pixels (default 0) */
  haloWidth: Styled<number, P>;
  /** Side of the text at the anchor (default `'center'`) */
  anchor: Anchor;
  /** Shift, ems (default [0, 0]) */
  offset: [x: number, y: number];
  /** Wrapping width, ems (default 10) */
  maxWidth: number;
  /** Ems (default 1.2) */
  lineHeight: number;
  /** Shown even where it overlaps other labels, and hiding none (MapLibre's allow-overlap and ignore-placement)
   * (default false) */
  overlap: boolean;
}

export interface SymbolOptions<P = Properties> extends Partial<SymbolStyle<P>>, Placement {
  data: FeatureData<P>;
}

/** Where a geometry is labelled: its points, the middle of its longest line, the centroid of its largest polygon */
function anchors(geometry: Geometry | null): Position[] {
  if (!geometry) return [];
  switch (geometry.type) {
    case 'Point':
      return [geometry.coordinates];
    case 'MultiPoint':
      return geometry.coordinates;
    case 'LineString':
    case 'MultiLineString': {
      const lines = geometry.type === 'LineString' ? [geometry.coordinates] : geometry.coordinates;
      const length = (l: Position[]) =>
        l.slice(1).reduce((s, p, i) => s + Math.hypot(p[0] - (l[i]?.[0] ?? 0), p[1] - (l[i]?.[1] ?? 0)), 0);
      const line = lines.reduce((a, b) => (length(b) > length(a) ? b : a), lines[0] ?? []);
      let rest = length(line) / 2;
      for (let i = 1; i < line.length; i += 1) {
        const [a, b] = [line[i - 1], line[i]];
        if (!a || !b) continue;
        const d = Math.hypot(b[0] - a[0], b[1] - a[1]);
        if (d >= rest && d > 0) return [[a[0] + ((b[0] - a[0]) * rest) / d, a[1] + ((b[1] - a[1]) * rest) / d]];
        rest -= d;
      }
      return line[0] ? [line[0]] : [];
    }
    case 'Polygon':
    case 'MultiPolygon': {
      const polygons = geometry.type === 'Polygon' ? [geometry.coordinates] : geometry.coordinates;
      let best: Position | undefined;
      let bestArea = 0;
      for (const p of polygons) {
        const ring = p[0] ?? [];
        let [a, cx, cy] = [0, 0, 0];
        for (let i = 0; i + 1 < ring.length; i += 1) {
          const [x0, y0] = ring[i] ?? [0, 0];
          const [x1, y1] = ring[i + 1] ?? [0, 0];
          const cross = x0 * y1 - x1 * y0;
          a += cross;
          cx += (x0 + x1) * cross;
          cy += (y0 + y1) * cross;
        }
        if (Math.abs(a) > bestArea) {
          bestArea = Math.abs(a);
          best = [cx / (3 * a), cy / (3 * a)];
        }
      }
      return best ? [best] : [];
    }
    case 'GeometryCollection':
      return geometry.geometries.flatMap(anchors);
  }
}

const MAX_LAT = 85.051128779806589;
const merc = ([lon, lat]: Position): [number, number] => {
  const phi = (Math.max(-MAX_LAT, Math.min(MAX_LAT, lat)) * Math.PI) / 180;
  return [(lon + 180) / 360, 0.5 - Math.log(Math.tan(Math.PI / 4 + phi / 2)) / (2 * Math.PI)];
};

/** The map's glyphs, for a layer */
export interface GlyphHost extends LayerHost {
  readonly glyphs: Glyphs;
}

export class SymbolLayer<P = Properties> extends Layer {
  private list: Feature<P>[];
  /** Bumped by each change: a shaping that finished after a newer one began is dropped */
  private generation = 0;
  private labels:
    | { anchors: Float64Array; boxes: Float32Array; styles: Float32Array; quads: Float32Array; feature: number[] }
    | undefined;

  constructor(
    private readonly host: GlyphHost,
    data: FeatureData<P>,
    private style: SymbolStyle<P>,
  ) {
    super(host);
    this.list = featuresOf(data);
  }

  get features(): readonly Feature<P>[] {
    return this.list;
  }

  setData(data: FeatureData<P>): void {
    this.list = featuresOf(data);
    void this.update();
  }

  setStyle(style: Partial<SymbolStyle<P>>): void {
    this.style = { ...this.style, ...style };
    void this.update();
  }

  setFilter(filter: ((properties: P, index: number) => boolean) | undefined): void {
    this.setStyle({ filter } as Partial<SymbolStyle<P>>);
  }

  setVisible(visible: boolean): void {
    this.style = { ...this.style, visible };
    this.sendLayer(this.map.engine, this.id);
    this.map.requestRender();
  }

  moveBefore(other: Layer | undefined): void {
    this.map.moveLayer(this, other);
  }

  /** Shapes the labels (loading their glyphs first) and sends them */
  private async update(): Promise<void> {
    const generation = ++this.generation;
    const { text, font, filter } = this.style;
    const parts = this.list.map((f, i): TextPart[] => {
      if (filter && !filter(f.properties, i)) return [];
      const t = evaluate(text, f.properties, i);
      return typeof t === 'string' ? [{ text: t }] : t;
    });
    const byFont = new Map<string, Set<number>>();
    for (const p of parts.flat()) {
      const key = p.font ?? font;
      const set = byFont.get(key) ?? new Set<number>();
      for (const code of codes([p])) set.add(code);
      byFont.set(key, set);
    }
    const loaded = new Map<string, Map<number, Glyph>>();
    await Promise.all(
      [...byFont].map(async ([f, set]) => {
        loaded.set(f, await this.host.glyphs.load(f, set));
      }),
    );
    if (generation !== this.generation) return;
    this.labels = this.build(parts, loaded);
    this.send(this.map.engine, this.id);
    this.map.requestRender();
  }

  private build(parts: TextPart[][], loaded: Map<string, Map<number, Glyph>>) {
    const { font, size, anchor, offset, maxWidth, lineHeight, color, haloColor, haloWidth, overlap } = this.style;
    // Shaped at the largest size; the engine scales by the zoom stops
    const shaped = typeof size === 'number' ? size : Math.max(...size.map(([, v]) => v), 1);
    const anchorList: number[] = [];
    const boxes: number[] = [];
    const styles: number[] = [];
    const quads: number[] = [];
    const feature: number[] = [];
    this.list.forEach((f, i) => {
      const text = parts[i];
      if (!text?.length || text.every((p) => !p.text)) return;
      const s = shape(text, (name) => loaded.get(name), { font, size: shaped, maxWidth, lineHeight, anchor, offset });
      if (!s.quads.length) return;
      const halo = evaluate(haloWidth, f.properties, i);
      const style = [
        ...parseColor(evaluate(color, f.properties, i)),
        ...parseColor(evaluate(haloColor, f.properties, i)),
        halo,
        overlap ? 1 : 0,
        0,
        0,
      ];
      for (const position of anchors(f.geometry)) {
        const label = feature.length;
        feature.push(i);
        anchorList.push(...merc(position));
        // The collision box: the ink, with the halo and a little room
        const pad = halo + 1;
        boxes.push(s.box[0] - pad, s.box[1] - pad, s.box[2] + pad, s.box[3] + pad);
        styles.push(...style);
        for (const q of s.quads) quads.push(label, ...q);
      }
    });
    return {
      anchors: Float64Array.from(anchorList),
      boxes: Float32Array.from(boxes),
      styles: Float32Array.from(styles),
      quads: Float32Array.from(quads),
      feature,
      shaped,
    };
  }

  private send(engine: Engine, id: number): void {
    const l = this.labels;
    if (l) engine.setSymbols(id, l.anchors, l.boxes, l.styles, l.quads);
    this.sendLayer(engine, id);
  }

  private sendLayer(engine: Engine, id: number): void {
    const { size, opacity, minZoom, maxZoom, visible } = this.style;
    const shaped = typeof size === 'number' ? size : Math.max(...size.map(([, v]) => v), 1);
    const scale: Size = typeof size === 'number' ? 1 : size.map(([z, v]): [number, number] => [z, v / shaped]);
    engine.setSymbolLayer(
      id,
      Float32Array.from(stops(scale)),
      Float32Array.from(stops(opacity)),
      minZoom,
      maxZoom,
      visible,
    );
  }

  protected create(engine: Engine): number {
    const id = engine.addSymbols();
    if (this.labels) this.send(engine, id);
    else {
      this.sendLayer(engine, id);
      // The first shaping, once the id is known
      queueMicrotask(() => void this.update());
    }
    return id;
  }
}

export function symbolStyle<P>(options: SymbolOptions<P>): SymbolStyle<P> {
  const { data: _, before: __, ...rest } = options;
  return {
    opacity: 1,
    minZoom: 0,
    maxZoom: 24,
    visible: true,
    filter: undefined,
    text: '',
    font: 'Source Sans Pro Regular',
    size: 12,
    color: '#000',
    haloColor: '#fff',
    haloWidth: 0,
    anchor: 'center',
    offset: [0, 0],
    maxWidth: 10,
    lineHeight: 1.2,
    overlap: false,
    ...rest,
  };
}
