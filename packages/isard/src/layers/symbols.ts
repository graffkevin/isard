/**
 * Text labels and icons from GeoJSON: at points, at the middle of lines, at the center of polygons. Shaped on the page,
 * placed by the engine every frame with collisions shared by all symbol layers (the top layer first, each layer in
 * data order), drawn with a halo, standing on the relief. The icons are the map's images (`map.addImage`, a vector
 * basemap's sprite), beside their text or stretched around it.
 */

import type { LayerHost } from '../types.ts';
import type { Anchor } from '../ui/overlay.ts';
import { toMercator } from '../util/geo.ts';
import type { Engine } from '../wasm/isard.js';
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
import { codes, DEFAULT_FONT, type Glyph, type Glyphs, type Shaping, shape, type TextPart } from './glyphs.ts';
import { type Icons, layoutIcon } from './icons.ts';
import { Layer } from './layers.ts';

export interface SymbolStyle<P = Properties> extends Omit<FeatureLayerStyle<P>, 'ramp'> {
  /** The text: a string (`\n` breaks lines) or runs with their own scale and font; empty: no label */
  text: Styled<string | TextPart[], P>;
  /** A font of the map's glyphs (default `Noto Sans Regular`; also Bold and Italic with the default glyphs) */
  font: Styled<string, P>;
  /** CSS pixels, or zoom stops (default 12) */
  size: Styled<Size, P>;
  /** `'point'` (default): horizontal text; `'line'`: along the line at its middle, kept upright */
  placement: Styled<'point' | 'line', P>;
  color: Styled<string, P>;
  haloColor: Styled<string, P>;
  /** CSS pixels (default 0) */
  haloWidth: Styled<number, P>;
  /** Side of the text at the anchor (default `'center'`) */
  anchor: Styled<Anchor, P>;
  /** Shift, ems (default [0, 0]) */
  offset: Styled<[x: number, y: number], P>;
  /** Wrapping width, ems (default 10) */
  maxWidth: Styled<number, P>;
  /** Ems (default 1.2) */
  lineHeight: Styled<number, P>;
  /** Shown even where it overlaps other labels, and hiding none (MapLibre's allow-overlap and ignore-placement);
   * `'allow'`: shown even where it overlaps, still hiding the labels after it; `'ignore'`: shown only where it fits,
   * hiding none (default false) */
  overlap: Styled<Overlap, P>;
  /** Shown without its icon where the icon does not fit (default false) */
  textOptional: Styled<boolean, P>;
  /** An image of the map (`map.addImage`); empty: none (default) */
  icon: Styled<string, P>;
  /** Scale of the image, or zoom stops (default 1) */
  iconSize: Styled<Size, P>;
  /** Side of the icon at the anchor (default `'center'`) */
  iconAnchor: Styled<Anchor, P>;
  /** Shift, CSS pixels at size 1 (default [0, 0]) */
  iconOffset: Styled<[x: number, y: number], P>;
  /** Degrees, clockwise (default 0) */
  iconRotate: Styled<number, P>;
  /** `'viewport'`: facing the screen; `'map'`: turning with the map, along its line when placed on one; `'auto'`
   * (default): `'map'` with the `'line'` placement */
  iconRotationAlignment: Styled<'map' | 'viewport' | 'auto', P>;
  /** As `overlap`, for the icon (default false) */
  iconOverlap: Styled<Overlap, P>;
  /** Shown without its text where the text does not fit (default false) */
  iconOptional: Styled<boolean, P>;
  /** Stretched around its text: its width, its height, both (a road shield; `'none'` by default) */
  iconTextFit: Styled<'none' | 'width' | 'height' | 'both', P>;
  /** Room around the text of a fitted icon: top, right, bottom, left (CSS pixels, default 0) */
  iconTextFitPadding: Styled<[top: number, right: number, bottom: number, left: number], P>;
  /** Color of a signed distance field image (default black) */
  iconColor: Styled<string, P>;
  /** 0 to 1 (default 1) */
  iconOpacity: Styled<number, P>;
}

/** How a label's text or icon meets the others: `true` shown wherever and hiding none, `'allow'` shown wherever,
 * `'ignore'` hiding none, `false` neither (MapLibre's allow-overlap and ignore-placement) */
export type Overlap = boolean | 'allow' | 'ignore';

export interface SymbolOptions<P = Properties> extends Partial<SymbolStyle<P>>, Placement {
  data: FeatureData<P>;
}

/** Where a geometry is labelled: its points, the middle of its longest line (with the direction of the line there),
 * the centroid of its largest polygon */
const anchors = (geometry: Geometry | null): { at: Position; along?: [number, number] }[] => {
  return points(geometry).map((p) => (p.length > 2 ? { at: [p[0], p[1]], along: [p[2] ?? 0, p[3] ?? 0] } : { at: p }));
};

/** Anchors as positions, a line's carrying the direction (lon, lat, dlon, dlat) */
const points = (geometry: Geometry | null): Position[] => {
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

        if (d >= rest && d > 0) {
          return [[a[0] + ((b[0] - a[0]) * rest) / d, a[1] + ((b[1] - a[1]) * rest) / d, b[0] - a[0], b[1] - a[1]]];
        }
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
      return geometry.geometries.flatMap(points);
  }
};

/** The map's glyphs and icons, for a layer */
export interface GlyphHost extends LayerHost {
  readonly glyphs: Glyphs;
  readonly icons: Icons;
}

/** Flags of a label for the engine (`flag` in symbols.rs) */
const FLAG = {
  textOverlap: 1,
  textIgnore: 2,
  iconOverlap: 4,
  iconIgnore: 8,
  textOptional: 16,
  iconOptional: 32,
  text: 64,
  icon: 128,
  textAlong: 256,
  iconAlong: 512,
  iconUpright: 1024,
} as const;

/** Flags of an overlap: allowed to overlap, ignored by the placement */
const overlapFlags = (overlap: Overlap, allow: number, ignore: number): number => {
  if (overlap === true) return allow | ignore;
  if (overlap === 'allow') return allow;

  return overlap === 'ignore' ? ignore : 0;
};

/** Room around an icon's box, CSS pixels (MapLibre's default `icon-padding`) */
const ICON_PADDING = 2;

/** Labels built for the engine: an anchor (Mercator x, y), a collision box, a style per label; glyph quads whose first
 * number is their label's index */
interface Labels {
  anchors: Float64Array;
  boxes: Float32Array;
  styles: Float32Array;
  quads: Float32Array;
}

/** Numbers per quad: its label's index, its place, its atlas coordinates, its scale (a glyph's; -1 or -2: an icon),
 * its turn */
const QUAD = 11;

/** The labels of a group of features, kept while they stay valid: same style, the same index of the group's first
 * feature (unless the style reads no index), and their glyphs and icons complete or none added since */
interface Chunk extends Labels {
  count: number;
  style: number;
  base: number;
  glyphs: number;
  icons: number;
  complete: boolean;
}

/** Every character of a run has its glyph */
const present = (part: TextPart, loaded: Map<string, Map<number, Glyph>>): boolean => {
  const glyphs = loaded.get(part.font ?? DEFAULT_FONT);

  if (!glyphs) return false;
  for (const ch of part.text) if (ch !== '\n' && !glyphs.has(ch.codePointAt(0) ?? 32)) return false;

  return true;
};

/** The labels of chunks one after the other, the quads' label indices moved past those of the chunks before */
const concat = (chunks: readonly Chunk[]): Labels => {
  if (chunks.length === 1 && chunks[0]) return chunks[0];

  const length = (key: keyof Labels) => chunks.reduce((n, c) => n + c[key].length, 0);
  const out: Labels = {
    anchors: new Float64Array(length('anchors')),
    boxes: new Float32Array(length('boxes')),
    styles: new Float32Array(length('styles')),
    quads: new Float32Array(length('quads')),
  };
  const at = { anchors: 0, boxes: 0, styles: 0, quads: 0 };
  let labels = 0;

  for (const c of chunks) {
    out.anchors.set(c.anchors, at.anchors);
    out.boxes.set(c.boxes, at.boxes);
    out.styles.set(c.styles, at.styles);
    out.quads.set(c.quads, at.quads);
    if (labels > 0)
      for (let q = at.quads; q < at.quads + c.quads.length; q += QUAD) out.quads[q] = (out.quads[q] ?? 0) + labels;
    at.anchors += c.anchors.length;
    at.boxes += c.boxes.length;
    at.styles += c.styles.length;
    at.quads += c.quads.length;
    labels += c.count;
  }

  return out;
};

export class SymbolLayer<P = Properties> extends Layer {
  private list: Feature<P>[];
  /** The features in groups that do not change (`setGroups`): each group's labels kept between updates */
  private groups: readonly (readonly Feature<P>[])[] | undefined;
  private readonly chunks = new WeakMap<readonly Feature<P>[], Chunk>();
  /** Bumped by each change: a shaping that finished after a newer one began is dropped */
  private generation = 0;
  /** Bumped by each change of style: the labels kept are shaped again */
  private styleGeneration = 0;
  /** No style function takes the index of a feature: a group's labels do not depend on its place */
  private indexFree = true;
  private labels: Labels | undefined;

  /** @internal */
  constructor(
    private readonly host: GlyphHost,
    data: FeatureData<P>,
    private style: SymbolStyle<P>,
  ) {
    super(host);
    this.list = featuresOf(data);
    this.styled();
  }

  get features(): readonly Feature<P>[] {
    return this.list;
  }

  setData(data: FeatureData<P>): void {
    this.list = featuresOf(data);
    this.groups = undefined;
    void this.update();
  }

  /**
   * @internal The features as groups that are never changed once given (the labels of a vector tile and style layer),
   * in order: the labels of a group seen before are not shaped again
   */
  setGroups(groups: readonly (readonly Feature<P>[])[]): void {
    this.groups = groups;
    this.list = groups.flat();
    void this.update();
  }

  setStyle(style: Partial<SymbolStyle<P>>): void {
    this.style = { ...this.style, ...style };
    this.styled();
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

  private styled(): void {
    this.styleGeneration += 1;
    // A function of one parameter reads the properties only
    this.indexFree = Object.values(this.style).every((v) => typeof v !== 'function' || v.length < 2);
  }

  /** The runs of text of each feature of a group (none for a feature filtered out) */
  private parts(group: readonly Feature<P>[], base: number): TextPart[][] {
    const { text, font, filter } = this.style;

    return group.map((f, k): TextPart[] => {
      const i = base + k;

      if (filter && !filter(f.properties, i)) return [];

      const t = evaluate(text, f.properties, i);
      const own = evaluate(font, f.properties, i);

      // Each run in its font, the feature's by default
      return (typeof t === 'string' ? [{ text: t }] : t).map((p) => ({ ...p, font: p.font ?? own }));
    });
  }

  /** Shapes the labels (loading their glyphs first) and sends them; the groups' labels still valid are kept */
  private async update(): Promise<void> {
    const generation = ++this.generation;
    const groups = this.groups ?? [this.list];
    const keep = this.groups !== undefined;
    const glyphs = this.host.glyphs;
    const todo: { group: readonly Feature<P>[]; base: number; parts: TextPart[][] }[] = [];

    groups.reduce((base, group) => {
      const c = keep ? this.chunks.get(group) : undefined;
      const valid =
        c !== undefined &&
        c.style === this.styleGeneration &&
        (c.base === base || this.indexFree) &&
        (c.complete || (c.glyphs === glyphs.version && c.icons === this.host.icons.version));

      if (!valid) todo.push({ group, base, parts: this.parts(group, base) });

      return base + group.length;
    }, 0);

    const byFont = new Map<string, Set<number>>();

    todo.forEach(({ parts }) => {
      parts.flat().forEach((p) => {
        const key = p.font ?? DEFAULT_FONT;
        const set = byFont.get(key) ?? new Set<number>();

        codes([p]).forEach((code) => {
          set.add(code);
        });
        byFont.set(key, set);
      });
    });

    const loaded = new Map<string, Map<number, Glyph>>();

    await Promise.all(
      [...byFont].map(async ([f, set]) => {
        loaded.set(f, await glyphs.load(f, set));
      }),
    );
    if (generation !== this.generation) return;

    const built = new Map<readonly Feature<P>[], Chunk>();

    todo.forEach(({ group, base: b, parts }) => {
      const chunk = this.build(group, b, parts, loaded);

      built.set(group, chunk);
      if (keep) this.chunks.set(group, chunk);
    });
    this.labels = concat(groups.map((g) => built.get(g) ?? (this.chunks.get(g) as Chunk)));
    // The icons packed for them
    this.host.icons.flush();
    this.send(this.map.engine, this.id);
    this.map.requestRender();
  }

  /** The labels of a group of features, `base` the index of its first feature */
  private build(
    group: readonly Feature<P>[],
    base: number,
    parts: TextPart[][],
    loaded: Map<string, Map<number, Glyph>>,
  ): Chunk {
    const { font, size, anchor, offset, maxWidth, lineHeight, color, haloColor, haloWidth, overlap, placement } =
      this.style;
    const st = this.style;
    const icons = this.host.icons;
    const anchorList: number[] = [];
    const boxes: number[] = [];
    const styles: number[] = [];
    const iconQuads: number[] = [];
    const textQuads: number[] = [];
    let count = 0;
    let complete = true;

    group.forEach((f, k) => {
      const i = base + k;
      const p = f.properties;
      const text = parts[k];
      const filtered = st.filter !== undefined && !st.filter(p, i);
      const hasText = !!text?.length && text.some((t) => t.text);
      const iconName = filtered ? '' : evaluate(st.icon, p, i);
      const icon = iconName ? icons.get(iconName) : undefined;

      // An image not added yet: laid out again once more are
      if (iconName && !icon) complete = false;
      if (!hasText && !icon) return;
      // Glyphs still missing: shaped again once more glyphs are loaded
      if (complete && text && !text.every((t) => present(t, loaded))) complete = false;

      // Shaped at its largest size; the engine scales by the zoom stops
      const shapedAt = (v: Size) => (typeof v === 'number' ? v : Math.max(...v.map(([, x]) => x), 1e-6));
      const scaleOf = (v: Size, at: number): Size =>
        typeof v === 'number' ? 1 : v.map(([z, x]): [number, number] => [z, x / at]);
      const s0 = evaluate(size, p, i);
      const shaped = shapedAt(s0);
      const scale = scaleOf(s0, shaped);
      const shaping =
        hasText && text
          ? shape(text, (name) => loaded.get(name), {
              font: evaluate(font, p, i),
              size: shaped,
              maxWidth: evaluate(maxWidth, p, i),
              lineHeight: evaluate(lineHeight, p, i),
              anchor: evaluate(anchor, p, i),
              offset: evaluate(offset, p, i),
            })
          : undefined;
      const s: Shaping | undefined = shaping?.quads.length ? shaping : undefined;

      if (!s && !icon) return;

      const along = evaluate(placement, p, i) === 'line';
      const textFlags =
        overlapFlags(evaluate(overlap, p, i), FLAG.textOverlap, FLAG.textIgnore) |
        (evaluate(st.textOptional, p, i) ? FLAG.textOptional : 0) |
        (s ? FLAG.text : 0);
      // The icon: beside its text, or stretched around it (then scaled and turned with it)
      const { iconScale, iconBox, iconColor, quads, iconTurns, fitted, iconFlags } = ((): {
        iconScale: Size;
        iconBox: [number, number, number, number];
        iconColor: [number, number, number, number];
        quads: number[][];
        iconTurns: boolean;
        fitted: boolean;
        iconFlags: number;
      } => {
        const none = {
          iconScale: 1,
          iconBox: [0, 0, 0, 0] as [number, number, number, number],
          iconColor: [0, 0, 0, 0] as [number, number, number, number],
          quads: [],
          iconTurns: false,
          fitted: false,
          iconFlags: 0,
        };

        if (!icon) return none;

        const fit = evaluate(st.iconTextFit, p, i);
        const fitted = fit !== 'none' && s !== undefined;
        const i0 = evaluate(st.iconSize, p, i);
        const iconShaped = fitted ? 1 : shapedAt(i0);
        const iconScale = fitted ? scale : scaleOf(i0, iconShaped);
        const laid = layoutIcon(icon, {
          size: iconShaped,
          anchor: evaluate(st.iconAnchor, p, i),
          offset: evaluate(st.iconOffset, p, i),
          rotate: evaluate(st.iconRotate, p, i),
          fit,
          padding: evaluate(st.iconTextFitPadding, p, i),
          text: s?.layout,
        });
        const laidOut = { ...none, fitted, iconScale, quads: laid.quads };

        if (!laid.quads.length) return laidOut;

        const iconFlags =
          FLAG.icon |
          overlapFlags(evaluate(st.iconOverlap, p, i), FLAG.iconOverlap, FLAG.iconIgnore) |
          (evaluate(st.iconOptional, p, i) ? FLAG.iconOptional : 0);
        const alignment = evaluate(st.iconRotationAlignment, p, i);
        const [r, g, b, a] = parseColor(evaluate(st.iconColor, p, i));

        return {
          ...laidOut,
          iconFlags: iconFlags | (fitted ? FLAG.iconUpright : 0),
          iconTurns: alignment === 'map' || (alignment === 'auto' && along),
          iconBox: [
            laid.box[0] - ICON_PADDING,
            laid.box[1] - ICON_PADDING,
            laid.box[2] + ICON_PADDING,
            laid.box[3] + ICON_PADDING,
          ],
          iconColor: [r, g, b, a * Math.max(0, Math.min(1, evaluate(st.iconOpacity, p, i)))],
        };
      })();
      const flags = textFlags | iconFlags;

      if (!(flags & (FLAG.text | FLAG.icon))) return;

      const halo = s ? evaluate(haloWidth, p, i) : 0;
      const fill = parseColor(evaluate(color, p, i));
      const haloRgba = parseColor(evaluate(haloColor, p, i));
      // The collision box: the ink, with the halo and a little room
      const pad = halo + 1;
      const textBox = s ? [s.box[0] - pad, s.box[1] - pad, s.box[2] + pad, s.box[3] + pad] : [0, 0, 0, 0];

      for (const { at, along: line } of anchors(f.geometry)) {
        const label = count++;
        const [x, y] = toMercator(at);

        anchorList.push(x, y);

        // The text along its line, kept upright; the icon turning with the map (along its line, or with the east at a
        // point), or with its text when fitted to it
        const textAlong = along && line !== undefined && s !== undefined;
        const iconAlong = (flags & FLAG.icon) !== 0 && (fitted ? textAlong : iconTurns);
        // The direction of its line in Mercator (y towards the south)
        const end = line && (textAlong || iconAlong) ? toMercator([at[0] + line[0], at[1] + line[1]]) : undefined;
        const direction: [number, number] = end ? [end[0] - x, end[1] - y] : [iconAlong ? 1 : 0, 0];
        const alongFlags = (textAlong ? FLAG.textAlong : 0) | (iconAlong ? FLAG.iconAlong : 0);
        const labelFlags = flags | (direction[0] === 0 && direction[1] === 0 ? 0 : alongFlags);

        boxes.push(...textBox);
        styles.push(
          ...fill,
          ...haloRgba,
          halo,
          labelFlags,
          ...direction,
          ...stops(scale),
          ...iconBox,
          ...iconColor,
          ...stops(iconScale),
        );
        for (const q of quads) iconQuads.push(label, ...q);
        if (s) for (const q of s.quads) textQuads.push(label, ...q, 0);
      }
    });

    return {
      anchors: Float64Array.from(anchorList),
      boxes: Float32Array.from(boxes),
      styles: Float32Array.from(styles),
      // The icons under the texts
      quads: Float32Array.from([...iconQuads, ...textQuads]),
      count,
      style: this.styleGeneration,
      base,
      glyphs: this.host.glyphs.version,
      icons: icons.version,
      complete,
    };
  }

  private send(engine: Engine, id: number): void {
    const l = this.labels;

    if (l) engine.setSymbols(id, l.anchors, l.boxes, l.styles, l.quads);
    this.sendLayer(engine, id);
  }

  private sendLayer(engine: Engine, id: number): void {
    const { opacity, minZoom, maxZoom, visible } = this.style;

    // The sizes are per label; the layer's own scale stays 1
    engine.setSymbolLayer(
      id,
      Float32Array.from(stops(1)),
      Float32Array.from(stops(opacity)),
      minZoom,
      maxZoom,
      visible,
    );
  }

  /** @internal */
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

export const symbolStyle = <P>(options: SymbolOptions<P>): SymbolStyle<P> => {
  const { data: _, before: __, ...rest } = options;

  return {
    opacity: 1,
    minZoom: 0,
    maxZoom: 24,
    visible: true,
    filter: undefined,
    text: '',
    font: DEFAULT_FONT,
    size: 12,
    color: '#000',
    haloColor: '#fff',
    haloWidth: 0,
    anchor: 'center',
    placement: 'point',
    offset: [0, 0],
    maxWidth: 10,
    lineHeight: 1.2,
    overlap: false,
    textOptional: false,
    icon: '',
    iconSize: 1,
    iconAnchor: 'center',
    iconOffset: [0, 0],
    iconRotate: 0,
    iconRotationAlignment: 'auto',
    iconOverlap: false,
    iconOptional: false,
    iconTextFit: 'none',
    iconTextFitPadding: [0, 0, 0, 0],
    iconColor: '#000',
    iconOpacity: 1,
    ...rest,
  };
};
