/**
 * Feature layers: lines, circles and fills from GeoJSON, styled per feature. A style value is a constant, zoom stops
 * (`[[zoom, value], …]`, linear in between, as MapLibre's `interpolate linear zoom`), or a function of the feature
 * returning either, evaluated once per feature when the data or the style change: the engine keeps the result in
 * textures, and restyling, recoloring by values or filtering never touches the geometry.
 */

import { Layer } from './layers.ts';
import type { LayerHost } from './types.ts';
import type { Engine } from './wasm/isard.js';

// ── GeoJSON (the parts used here) ──

export type Position = [lon: number, lat: number, ...rest: number[]];
export type Geometry =
  | { type: 'Point'; coordinates: Position }
  | { type: 'MultiPoint'; coordinates: Position[] }
  | { type: 'LineString'; coordinates: Position[] }
  | { type: 'MultiLineString'; coordinates: Position[][] }
  | { type: 'Polygon'; coordinates: Position[][] }
  | { type: 'MultiPolygon'; coordinates: Position[][][] }
  | { type: 'GeometryCollection'; geometries: Geometry[] };
export type Properties = Record<string, unknown>;
export interface Feature<P = Properties> {
  type: 'Feature';
  geometry: Geometry | null;
  properties: P;
  id?: string | number;
}
export interface FeatureCollection<P = Properties> {
  type: 'FeatureCollection';
  features: Feature<P>[];
}
/** What a feature layer takes: a collection, its features, or one feature */
export type FeatureData<P = Properties> = FeatureCollection<P> | Feature<P>[] | Feature<P>;

// ── Style values ──

/** Zoom stops: `[[zoom, value], …]` in increasing zooms, up to four, linear in between, constant outside */
export type Stops = [zoom: number, value: number][];
/** A size: CSS pixels, or zoom stops */
export type Size = number | Stops;
/** A value for all features, or a function of each feature (its properties and its index in the data) */
export type Styled<T, P = Properties> = T | ((properties: P, index: number) => T);
/** Colors of values: `[[value, color], …]` in increasing values, linear in between */
export type Ramp = [value: number, color: string][];

export interface FeatureLayerStyle<P = Properties> {
  /** Layer opacity, 0 to 1, or zoom stops (a layer fading in past a zoom) (default 1) */
  opacity: Size;
  /** Zooms the layer shows in: from `minZoom` (included) to `maxZoom` (excluded) */
  minZoom: number;
  maxZoom: number;
  visible: boolean;
  /** Features drawn; the others hidden (default all) */
  filter: ((properties: P, index: number) => boolean) | undefined;
  /** Colors of the values given by `setValues`: a feature with a value takes its color here, the others keep theirs */
  ramp: Ramp | undefined;
}

export interface LineStyle<P = Properties> extends FeatureLayerStyle<P> {
  /** CSS color: `#rgb`, `#rrggbb`, `#rrggbbaa`, `rgb()`, `rgba()` (default `#000`) */
  color: Styled<string, P>;
  /** CSS pixels (default 1) */
  width: Styled<Size, P>;
  /** Dash and gap, in line widths (MapLibre's `line-dasharray`); undefined: solid */
  dash: [dash: number, gap: number] | undefined;
}

export interface CircleStyle<P = Properties> extends FeatureLayerStyle<P> {
  color: Styled<string, P>;
  /** CSS pixels (default 5) */
  radius: Styled<Size, P>;
  strokeColor: Styled<string, P>;
  /** CSS pixels (default 0) */
  strokeWidth: Styled<Size, P>;
  /** `'viewport'` (default): discs facing the screen; `'map'`: lying on the ground (MapLibre's
   * `circle-pitch-alignment`) */
  alignment: 'viewport' | 'map';
}

export interface FillStyle<P = Properties> extends FeatureLayerStyle<P> {
  color: Styled<string, P>;
}

/** Where a new layer goes: under `before`, or on top of the others */
export interface Placement {
  before?: Layer;
}

export interface LineOptions<P = Properties> extends Partial<LineStyle<P>>, Placement {
  /** LineString and MultiLineString features; the rings of Polygon and MultiPolygon features are drawn as lines */
  data: FeatureData<P>;
}
export interface CircleOptions<P = Properties> extends Partial<CircleStyle<P>>, Placement {
  /** Point and MultiPoint features */
  data: FeatureData<P>;
}
export interface FillOptions<P = Properties> extends Partial<FillStyle<P>>, Placement {
  /** Polygon and MultiPolygon features */
  data: FeatureData<P>;
}

const BASE: FeatureLayerStyle = {
  opacity: 1,
  minZoom: 0,
  maxZoom: 24,
  visible: true,
  filter: undefined,
  ramp: undefined,
};

// ── Colors ──

const NAMED: Record<string, [number, number, number, number]> = {
  black: [0, 0, 0, 1],
  white: [1, 1, 1, 1],
  transparent: [0, 0, 0, 0],
  red: [1, 0, 0, 1],
  blue: [0, 0, 1, 1],
  grey: [0.5, 0.5, 0.5, 1],
  gray: [0.5, 0.5, 0.5, 1],
};

/** A CSS color to RGBA 0-1 (hex forms, `rgb()`, `rgba()`, a few names); black if unknown */
export function parseColor(color: string): [r: number, g: number, b: number, a: number] {
  const c = color.trim().toLowerCase();
  const named = NAMED[c];
  if (named) return [...named];
  if (c.startsWith('#')) {
    const hex = c.slice(1);
    const full = hex.length <= 4 ? [...hex].map((h) => h + h).join('') : hex;
    const v = (i: number) => Number.parseInt(full.slice(i, i + 2), 16) / 255;
    return [v(0), v(2), v(4), full.length >= 8 ? v(6) : 1];
  }
  const match = /^rgba?\(([^)]*)\)$/.exec(c);
  if (match?.[1]) {
    const parts = match[1].split(/[\s,/]+/).filter(Boolean);
    const channel = (p: string | undefined) => {
      if (p === undefined) return 1;
      return p.endsWith('%') ? Number.parseFloat(p) / 100 : Number.parseFloat(p);
    };
    const [r, g, b] = parts.slice(0, 3).map((p) => channel(p) / (p.endsWith('%') ? 1 : 255));
    return [r ?? 0, g ?? 0, b ?? 0, channel(parts[3])];
  }
  return [0, 0, 0, 1];
}

/** 256 RGBA texels of a ramp over its value range, and that range */
export function rampTexels(ramp: Ramp): { rgba: Uint8Array; min: number; max: number } {
  const stops = ramp.map(([v, c]) => [v, parseColor(c)] as const).sort((a, b) => a[0] - b[0]);
  const min = stops[0]?.[0] ?? 0;
  const max = stops.at(-1)?.[0] ?? 1;
  const rgba = new Uint8Array(256 * 4);
  for (let i = 0; i < 256; i += 1) {
    const v = min + ((max - min) * i) / 255;
    let k = 0;
    while (k + 1 < stops.length && (stops[k + 1]?.[0] ?? 0) < v) k += 1;
    const [v0, c0] = stops[k] ?? [min, [0, 0, 0, 1]];
    const [v1, c1] = stops[k + 1] ?? [v0, c0];
    const t = v1 > v0 ? Math.max(0, Math.min(1, (v - v0) / (v1 - v0))) : 0;
    for (let j = 0; j < 4; j += 1)
      rgba[i * 4 + j] = Math.round(((c0[j] ?? 0) + ((c1[j] ?? 0) - (c0[j] ?? 0)) * t) * 255);
  }
  return { rgba, min, max };
}

/** Up to four stops as zooms then values, constant if a number */
export function stops(size: Size): [number, number, number, number, number, number, number, number] {
  if (!Array.isArray(size)) {
    // A style function may return nothing for a feature without the property: 0
    const v = Number.isFinite(size) ? size : 0;
    return [0, 0, 0, 0, v, v, v, v];
  }
  // More than four: resampled at four zooms over their range
  let s = size;
  if (s.length > 4) {
    const z0 = s[0]?.[0] ?? 0;
    const z1 = s.at(-1)?.[0] ?? 0;
    s = [0, 1, 2, 3].map((k): [number, number] => {
      const z = z0 + ((z1 - z0) * k) / 3;
      return [z, interpolate(size, z)];
    });
  }
  const at = (k: number) => s[Math.min(k, s.length - 1)] ?? [0, 0];
  return [at(0)[0], at(1)[0], at(2)[0], at(3)[0], at(0)[1], at(1)[1], at(2)[1], at(3)[1]];
}

/** Value of zoom stops at a zoom */
export function interpolate(size: Size, zoom: number): number {
  if (typeof size === 'number') return size;
  const first = size[0];
  if (!first) return 0;
  if (zoom <= first[0]) return first[1];
  for (let k = 1; k < size.length; k += 1) {
    const [z0, v0] = size[k - 1] ?? first;
    const [z1, v1] = size[k] ?? first;
    if (zoom <= z1) return v0 + ((v1 - v0) * (zoom - z0)) / Math.max(z1 - z0, 1e-9);
  }
  return size.at(-1)?.[1] ?? 0;
}

export function evaluate<T, P>(value: Styled<T, P>, properties: P, index: number): T {
  return typeof value === 'function' ? (value as (p: P, i: number) => T)(properties, index) : value;
}

// ── Geometry ──

const MAX_LAT = 85.051128779806589;
const mercX = (lon: number) => (lon + 180) / 360;
const mercY = (lat: number) => {
  const phi = (Math.max(-MAX_LAT, Math.min(MAX_LAT, lat)) * Math.PI) / 180;
  return 0.5 - Math.log(Math.tan(Math.PI / 4 + phi / 2)) / (2 * Math.PI);
};

export function featuresOf<P>(data: FeatureData<P>): Feature<P>[] {
  if (Array.isArray(data)) return data;
  return data.type === 'FeatureCollection' ? data.features : [data];
}

/** Every geometry of a feature, collections flattened */
function* geometries(geometry: Geometry | null): Generator<Geometry> {
  if (!geometry) return;
  if (geometry.type === 'GeometryCollection') for (const g of geometry.geometries) yield* geometries(g);
  else yield geometry;
}

/** A growable typed array */
class Grow<T extends Float64Array | Uint32Array> {
  length = 0;
  constructor(public data: T) {}
  push(...values: number[]): void {
    if (this.length + values.length > this.data.length) {
      const next = new (this.data.constructor as new (n: number) => T)(
        Math.max(16, this.data.length * 2, this.length + values.length),
      );
      next.set(this.data);
      this.data = next;
    }
    for (const v of values) this.data[this.length++] = v;
  }
  done(): T {
    return this.data.subarray(0, this.length) as T;
  }
}

/** Lines of the features: points (Mercator), first point of each line, feature of each line */
function lineGeometry(features: Feature<unknown>[]) {
  const points = new Grow(new Float64Array(1024));
  const start = new Grow(new Uint32Array(64));
  const feature = new Grow(new Uint32Array(64));
  const line = (coords: Position[], f: number) => {
    start.push(points.length / 2);
    feature.push(f);
    for (const [lon, lat] of coords) points.push(mercX(lon), mercY(lat));
  };
  features.forEach((f, i) => {
    for (const g of geometries(f.geometry)) {
      if (g.type === 'LineString') line(g.coordinates, i);
      else if (g.type === 'MultiLineString') for (const l of g.coordinates) line(l, i);
      else if (g.type === 'Polygon') for (const r of g.coordinates) line(r, i);
      else if (g.type === 'MultiPolygon') for (const p of g.coordinates) for (const r of p) line(r, i);
    }
  });
  start.push(points.length / 2);
  return { points: points.done(), start: start.done(), feature: feature.done() };
}

function pointGeometry(features: Feature<unknown>[]) {
  const points = new Grow(new Float64Array(1024));
  const feature = new Grow(new Uint32Array(64));
  const point = ([lon, lat]: Position, f: number) => {
    points.push(mercX(lon), mercY(lat));
    feature.push(f);
  };
  features.forEach((f, i) => {
    for (const g of geometries(f.geometry)) {
      if (g.type === 'Point') point(g.coordinates, i);
      else if (g.type === 'MultiPoint') for (const p of g.coordinates) point(p, i);
    }
  });
  return { points: points.done(), feature: feature.done() };
}

export function polygonGeometry(features: Feature<unknown>[]) {
  const points = new Grow(new Float64Array(1024));
  const ringStart = new Grow(new Uint32Array(64));
  const polygonStart = new Grow(new Uint32Array(64));
  const feature = new Grow(new Uint32Array(64));
  const polygon = (rings: Position[][], f: number) => {
    polygonStart.push(ringStart.length);
    feature.push(f);
    for (const ring of rings) {
      ringStart.push(points.length / 2);
      for (const [lon, lat] of ring) points.push(mercX(lon), mercY(lat));
    }
  };
  features.forEach((f, i) => {
    for (const g of geometries(f.geometry)) {
      if (g.type === 'Polygon') polygon(g.coordinates, i);
      else if (g.type === 'MultiPolygon') for (const p of g.coordinates) polygon(p, i);
    }
  });
  ringStart.push(points.length / 2);
  polygonStart.push(ringStart.length - 1);
  return {
    points: points.done(),
    ringStart: ringStart.done(),
    polygonStart: polygonStart.done(),
    feature: feature.done(),
  };
}

// ── Layers ──

/** Floats per feature in the engine's style (see `setFeatureStyle`) */
export const FLOATS = 32;

/** What every feature layer shares: its data, its style in the engine, values, filter */
export abstract class FeatureLayer<P, S extends FeatureLayerStyle<P>> extends Layer {
  protected list: Feature<P>[];
  protected values: Float32Array | undefined;

  constructor(
    map: LayerHost,
    data: FeatureData<P>,
    protected style: S,
  ) {
    super(map);
    this.list = featuresOf(data);
  }

  /** The features, in the order of the data (their index is what `pick` returns) */
  get features(): readonly Feature<P>[] {
    return this.list;
  }

  /** The feature of a picked index */
  featureAt(index: number): Feature<P> | undefined {
    return this.list[index];
  }

  /** Replaces the features; the style is evaluated again for them */
  setData(data: FeatureData<P>): void {
    const before = this.list.length;
    this.list = featuresOf(data);
    if (this.list.length !== before) this.values = undefined;
    // A new feature count needs new textures: the layer is created again at its place
    if (this.list.length !== before) this.recreate();
    else {
      this.upload(this.map.engine, this.id);
      this.applyStyle(this.map.engine, this.id);
    }
    this.map.requestRender();
  }

  setStyle(style: Partial<S>): void {
    this.style = { ...this.style, ...style };
    this.applyStyle(this.map.engine, this.id);
    this.map.requestRender();
  }

  /** One value per feature (in data order), colored through the style's `ramp`; NaN keeps a feature's color */
  setValues(values: Float32Array | undefined): void {
    this.values = values;
    this.map.engine.setFeatureValues(this.id, values ?? new Float32Array(this.list.length).fill(Number.NaN));
    this.map.requestRender();
  }

  /** Shows only the features the predicate keeps; undefined shows them all */
  setFilter(filter: ((properties: P, index: number) => boolean) | undefined): void {
    this.style = { ...this.style, filter };
    this.map.engine.setFeatureMask(this.id, this.mask());
    this.map.requestRender();
  }

  setVisible(visible: boolean): void {
    this.setStyle({ visible } as Partial<S>);
  }

  private mask(): Uint8Array {
    const { filter } = this.style;
    const mask = new Uint8Array(this.list.length).fill(255);
    if (filter)
      this.list.forEach((f, i) => {
        if (!filter(f.properties, i)) mask[i] = 0;
      });
    return mask;
  }

  /** Recreated in the engine at the same place (a new feature count) */
  private recreate(): void {
    const engine = this.map.engine;
    const old = this.id;
    this.attach(engine);
    this.map.replaceLayer(this, old);
  }

  protected create(engine: Engine): number {
    const id = this.add(engine);
    this.applyStyle(engine, id);
    if (this.values) engine.setFeatureValues(id, this.values);
    return id;
  }

  /** Adds the geometry to the engine, returns the layer id */
  protected abstract add(engine: Engine): number;
  /** Sends the geometry again (same feature count) */
  protected abstract upload(engine: Engine, id: number): void;
  /** Two colors and two sizes of a feature */
  protected abstract featureStyle(
    properties: P,
    index: number,
  ): [color: string, second: string, size: Size, second: Size];
  /** Layer-specific settings (dashes, alignment) */
  protected abstract applyExtra(engine: Engine, id: number): void;

  protected applyStyle(engine: Engine, id: number): void {
    const out = new Float32Array(this.list.length * FLOATS);
    this.list.forEach((f, i) => {
      const [color, second, size, size2] = this.featureStyle(f.properties, i);
      const o = i * FLOATS;
      out.set(parseColor(color), o);
      out.set(parseColor(second), o + 4);
      out.set(stops(size), o + 8);
      out.set(stops(size2), o + 16);
    });
    engine.setFeatureStyle(id, out);
    engine.setFeatureMask(id, this.mask());
    const { ramp, opacity, minZoom, maxZoom, visible } = this.style;
    const r = ramp && rampTexels(ramp);
    engine.setFeatureRamp(id, r?.rgba, r?.min ?? 0, r?.max ?? 1);
    engine.setFeatureLayer(id, Float32Array.from(stops(opacity)), minZoom, maxZoom, visible);
    this.applyExtra(engine, id);
  }
}

export class LineLayer<P = Properties> extends FeatureLayer<P, LineStyle<P>> {
  protected add(engine: Engine): number {
    const g = lineGeometry(this.list);
    return engine.addLines(g.points, g.start, g.feature, this.list.length);
  }

  protected upload(engine: Engine, id: number): void {
    const g = lineGeometry(this.list);
    engine.setLines(id, g.points, g.start, g.feature);
  }

  protected featureStyle(properties: P, index: number): [string, string, Size, Size] {
    const { color, width } = this.style;
    return [evaluate(color, properties, index), '#000', evaluate(width, properties, index), 0];
  }

  protected applyExtra(engine: Engine, id: number): void {
    const [dash, gap] = this.style.dash ?? [0, 0];
    engine.setLineDash(id, dash, gap);
  }
}

export class CircleLayer<P = Properties> extends FeatureLayer<P, CircleStyle<P>> {
  protected add(engine: Engine): number {
    const g = pointGeometry(this.list);
    return engine.addCircles(g.points, g.feature, this.list.length);
  }

  protected upload(engine: Engine, id: number): void {
    const g = pointGeometry(this.list);
    engine.setCircles(id, g.points, g.feature);
  }

  protected featureStyle(properties: P, index: number): [string, string, Size, Size] {
    const { color, strokeColor, radius, strokeWidth } = this.style;
    return [
      evaluate(color, properties, index),
      evaluate(strokeColor, properties, index),
      evaluate(radius, properties, index),
      evaluate(strokeWidth, properties, index),
    ];
  }

  protected applyExtra(engine: Engine, id: number): void {
    engine.setCircleAlignment(id, this.style.alignment === 'map');
  }
}

export class FillLayer<P = Properties> extends FeatureLayer<P, FillStyle<P>> {
  protected add(engine: Engine): number {
    const g = polygonGeometry(this.list);
    return engine.addFills(g.points, g.ringStart, g.polygonStart, g.feature, this.list.length);
  }

  protected upload(engine: Engine, id: number): void {
    const g = polygonGeometry(this.list);
    engine.setFills(id, g.points, g.ringStart, g.polygonStart, g.feature);
  }

  protected featureStyle(properties: P, index: number): [string, string, Size, Size] {
    return [evaluate(this.style.color, properties, index), '#000', 0, 0];
  }

  protected applyExtra(): void {}
}

export function lineStyle<P>(options: LineOptions<P>): LineStyle<P> {
  const { data: _, before: __, ...rest } = options;
  return { ...(BASE as FeatureLayerStyle<P>), color: '#000', width: 1, dash: undefined, ...rest };
}

export function circleStyle<P>(options: CircleOptions<P>): CircleStyle<P> {
  const { data: _, before: __, ...rest } = options;
  return {
    ...(BASE as FeatureLayerStyle<P>),
    color: '#000',
    radius: 5,
    strokeColor: '#fff',
    strokeWidth: 0,
    alignment: 'viewport',
    ...rest,
  };
}

export function fillStyle<P>(options: FillOptions<P>): FillStyle<P> {
  const { data: _, before: __, ...rest } = options;
  return { ...(BASE as FeatureLayerStyle<P>), color: '#000', ...rest };
}
