/**
 * The parts of the MapLibre style specification a vector basemap needs, evaluated on the page: filters (legacy
 * `["==", "key", value]` forms and expressions), property values given as constants, legacy functions (`{ stops }`
 * of zoom or of a property: exponential, interval, categorical, identity) or expressions (`get`, `has`, `concat`,
 * `interpolate`, `step`, `match`, `case`, `coalesce`, comparisons, `all`, `any`, `!`, `in`, `zoom`, arithmetic,
 * `format`, conversions and assertions, `upcase`, `downcase`, `literal`…; others give null, with a warning), colors
 * interpolated in RGBA, `{token}` text fields.
 */
import { parseCssColor } from './color.ts';
import type { TileFeature, Value } from './mvt.ts';

export type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

export interface StyleLayer {
  id: string;
  type: string;
  source?: string;
  'source-layer'?: string;
  minzoom?: number;
  maxzoom?: number;
  filter?: Json;
  layout?: Record<string, Json>;
  paint?: Record<string, Json>;
}

export interface Style {
  version: number;
  sources: Record<
    string,
    { type: string; tiles?: string[]; url?: string; maxzoom?: number; minzoom?: number; tileSize?: number }
  >;
  layers: StyleLayer[];
  glyphs?: string;
  sprite?: string;
}

/** A raster source of a style: its tile URL, its deepest level and the CSS pixels of its tiles (MapLibre's
 * `tileSize`: 512 by default) */
export interface RasterSource {
  tiles: string;
  maxzoom: number;
  tileSize: number;
}

/** A label to shape and place, in style terms evaluated at the tile's zoom */
export interface LabelCandidate {
  /** Index of its style layer: higher ones are placed first */
  layer: number;
  text: string;
  lon: number;
  lat: number;
  /** Along a line: the direction there (degrees of longitude and latitude) */
  along?: [number, number];
  font: string;
  size: Json;
  color: string;
  haloColor: string;
  haloWidth: number;
  anchor: string;
  offset: [number, number];
  maxWidth: number;
  lineHeight: number;
  overlap: boolean;
}

/** What expressions see of a feature */
export interface Context {
  zoom: number;
  properties: Record<string, Value>;
  /** 'Point', 'LineString' or 'Polygon' */
  type: string;
}

const TYPES = ['Unknown', 'Point', 'LineString', 'Polygon'];

export function context(feature: TileFeature, zoom: number): Context {
  return { zoom, properties: feature.properties, type: TYPES[feature.type] ?? 'Unknown' };
}

// ── Colors ──

/** A color to RGBA 0-1, undefined if it is not one */
export function color(value: unknown): [number, number, number, number] | undefined {
  return parseCssColor(value);
}

export function css([r, g, b, a]: [number, number, number, number]): string {
  return `rgba(${Math.round(r * 255)},${Math.round(g * 255)},${Math.round(b * 255)},${a})`;
}

// ── Interpolation ──

/** Factor between two stops: linear, or exponential of `base` */
function factor(x: number, x0: number, x1: number, base = 1): number {
  const d = x1 - x0;
  if (d === 0) return 0;
  const t = x - x0;
  return base === 1 ? t / d : (base ** t - 1) / (base ** d - 1);
}

function mix(a: unknown, b: unknown, t: number): unknown {
  if (typeof a === 'number' && typeof b === 'number') return a + (b - a) * t;
  const ca = color(a);
  const cb = color(b);
  if (ca && cb)
    return css(
      [0, 1, 2, 3].map((i) => (ca[i] ?? 0) + ((cb[i] ?? 0) - (ca[i] ?? 0)) * t) as [number, number, number, number],
    );
  if (Array.isArray(a) && Array.isArray(b)) return a.map((v, i) => mix(v, b[i], t));
  return t < 0.5 ? a : b;
}

// ── Legacy functions ──

interface LegacyFunction {
  stops: [Json, Json][];
  base?: number;
  type?: string;
  property?: string;
  default?: Json;
}

function isFunction(v: Json): v is { [key: string]: Json } & LegacyFunction {
  return typeof v === 'object' && v !== null && !Array.isArray(v) && Array.isArray((v as { stops?: Json }).stops);
}

function legacy(f: LegacyFunction, ctx: Context): unknown {
  const input = f.property === undefined ? ctx.zoom : ctx.properties[f.property];
  const stops = f.stops;
  const type =
    f.type ?? (f.property === undefined || typeof stops[0]?.[0] === 'number' ? 'exponential' : 'categorical');
  if (type === 'identity') return input ?? f.default;
  if (type === 'categorical') return stops.find(([k]) => k === input)?.[1] ?? f.default;
  if (typeof input !== 'number' || !stops.length) return f.default ?? stops[0]?.[1];
  // Zoom-and-property stops ({ zoom, value } keys) are taken at their zoom only
  const key = (s: [Json, Json]) =>
    typeof s[0] === 'object' && s[0] !== null ? Number((s[0] as { zoom?: number }).zoom) : Number(s[0]);
  const first = stops[0];
  if (!first) return f.default;
  if (input <= key(first)) return first[1];
  for (let i = 1; i < stops.length; i += 1) {
    const [s0, s1] = [stops[i - 1], stops[i]];
    if (!s0 || !s1) continue;
    if (input < key(s1)) {
      if (type === 'interval') return s0[1];
      return mix(s0[1], s1[1], factor(input, key(s0), key(s1), f.base ?? 1));
    }
  }
  return stops.at(-1)?.[1];
}

// ── Expressions ──

function get(ctx: Context, key: unknown): unknown {
  if (key === '$type') return ctx.type;
  return typeof key === 'string' ? ctx.properties[key] : undefined;
}

/** The operators `evaluate` knows */
const OPERATORS = new Set([
  'literal',
  'zoom',
  'get',
  'has',
  '!has',
  'properties',
  'id',
  'geometry-type',
  'concat',
  'format',
  'image',
  'number-format',
  'to-number',
  'to-string',
  'to-boolean',
  'to-color',
  'number',
  'string',
  'boolean',
  'typeof',
  'length',
  'at',
  'upcase',
  'downcase',
  'coalesce',
  '!',
  'all',
  'any',
  '==',
  '!=',
  '<',
  '<=',
  '>',
  '>=',
  'in',
  'case',
  'match',
  'step',
  'interpolate',
  'interpolate-hcl',
  'interpolate-lab',
  'rgb',
  'rgba',
  '+',
  '-',
  '*',
  '/',
  '%',
  '^',
  'min',
  'max',
  'abs',
  'round',
  'floor',
  'ceil',
  'sqrt',
  'ln',
  'log10',
  'log2',
  'pi',
  'e',
]);

/** Whether a value is an expression (an array starting with a known operator), not a literal array */
export function isExpression(e: unknown): e is Json[] {
  return Array.isArray(e) && typeof e[0] === 'string' && OPERATORS.has(e[0]);
}

/** Operators already warned about */
const warned = new Set<string>();

const str = (v: unknown) => (v === undefined || v === null ? '' : String(v));

export function evaluate(e: Json, ctx: Context): unknown {
  if (isFunction(e)) return legacy(e, ctx);
  if (!Array.isArray(e)) return e;
  const [op, ...args] = e;
  if (typeof op !== 'string') return e;
  const ev = (i: number) => evaluate(args[i] ?? null, ctx);
  const num = (i: number) => Number(ev(i));
  const nums = () => args.map((_, i) => num(i));
  switch (op) {
    case 'literal':
      return args[0];
    case 'zoom':
      return ctx.zoom;
    case 'get':
      return get(ctx, ev(0));
    case 'has':
      return get(ctx, ev(0)) !== undefined;
    case '!has':
      return get(ctx, ev(0)) === undefined;
    case 'properties':
      return ctx.properties;
    case 'id':
      return null;
    case 'geometry-type':
      return ctx.type;
    case 'concat':
      return args.map((_, i) => str(ev(i))).join('');
    case 'format':
      // Its text sections, the options of each (objects) left out
      return args.map((a, i) => (a !== null && typeof a === 'object' && !Array.isArray(a) ? '' : str(ev(i)))).join('');
    case 'image':
      return ev(0);
    case 'number-format': {
      const options = (args[1] ?? {}) as {
        locale?: string;
        'min-fraction-digits'?: number;
        'max-fraction-digits'?: number;
      };
      const n = num(0);
      if (!Number.isFinite(n)) return '';
      return n.toLocaleString(options.locale, {
        minimumFractionDigits: options['min-fraction-digits'],
        maximumFractionDigits: options['max-fraction-digits'],
      });
    }
    case 'to-number': {
      for (let i = 0; i < args.length; i += 1) {
        const v = ev(i);
        const n = typeof v === 'string' && v.trim() === '' ? Number.NaN : Number(v);
        if (v !== null && v !== undefined && !Number.isNaN(n)) return n;
      }
      return 0;
    }
    case 'to-string':
      return str(ev(0));
    case 'to-boolean': {
      const v = ev(0);
      return !(v === '' || v === 0 || v === false || v === null || v === undefined || Number.isNaN(v));
    }
    case 'to-color':
    case 'number':
    case 'string':
    case 'boolean': {
      // Assertions: the first value of the type, else the last one
      const types: Record<string, (v: unknown) => boolean> = {
        'to-color': (v) => color(v) !== undefined,
        number: (v) => typeof v === 'number',
        string: (v) => typeof v === 'string',
        boolean: (v) => typeof v === 'boolean',
      };
      for (let i = 0; i < args.length; i += 1) {
        const v = ev(i);
        if (types[op]?.(v)) return v;
      }
      return args.length ? ev(args.length - 1) : null;
    }
    case 'typeof': {
      const v = ev(0);
      if (v === null || v === undefined) return 'null';
      return Array.isArray(v) ? 'array' : typeof v;
    }
    case 'length': {
      const v = ev(0);
      return typeof v === 'string' || Array.isArray(v) ? v.length : 0;
    }
    case 'at': {
      const v = ev(1);
      return Array.isArray(v) ? (v[num(0)] ?? null) : null;
    }
    case 'upcase':
      return str(ev(0)).toUpperCase();
    case 'downcase':
      return str(ev(0)).toLowerCase();
    case 'coalesce':
      for (let i = 0; i < args.length; i += 1) {
        const v = ev(i);
        if (v !== undefined && v !== null) return v;
      }
      return null;
    case '!':
      return !ev(0);
    case 'all':
      return args.every((_, i) => Boolean(ev(i)));
    case 'any':
      return args.some((_, i) => Boolean(ev(i)));
    case '==':
      return ev(0) === ev(1);
    case '!=':
      return ev(0) !== ev(1);
    case '<':
      return (ev(0) as number) < (ev(1) as number);
    case '<=':
      return (ev(0) as number) <= (ev(1) as number);
    case '>':
      return (ev(0) as number) > (ev(1) as number);
    case '>=':
      return (ev(0) as number) >= (ev(1) as number);
    case 'in': {
      const haystack = ev(1);
      const needle = ev(0);
      if (typeof haystack === 'string') return haystack.includes(String(needle));
      return Array.isArray(haystack) && haystack.includes(needle as Json);
    }
    case 'case': {
      for (let i = 0; i + 1 < args.length; i += 2) if (ev(i)) return ev(i + 1);
      return ev(args.length - 1);
    }
    case 'match': {
      const input = ev(0);
      for (let i = 1; i + 1 < args.length; i += 2) {
        const label = args[i];
        if (Array.isArray(label) ? label.includes(input as Json) : label === input) return ev(i + 1);
      }
      return ev(args.length - 1);
    }
    case 'step': {
      const input = num(0);
      let out = ev(1);
      for (let i = 2; i + 1 < args.length; i += 2) {
        if (input >= Number(args[i])) out = ev(i + 1);
        else break;
      }
      return out;
    }
    case 'interpolate':
    case 'interpolate-hcl':
    case 'interpolate-lab': {
      const kind = args[0];
      const input = num(1);
      const base = Array.isArray(kind) && kind[0] === 'exponential' ? Number(kind[1]) : 1;
      const stops: [number, Json][] = [];
      for (let i = 2; i + 1 < args.length; i += 2) stops.push([Number(args[i]), args[i + 1] ?? null]);
      const first = stops[0];
      if (!first) return null;
      if (input <= first[0]) return evaluate(first[1], ctx);
      for (let i = 1; i < stops.length; i += 1) {
        const [s0, s1] = [stops[i - 1], stops[i]];
        if (s0 && s1 && input < s1[0])
          return mix(evaluate(s0[1], ctx), evaluate(s1[1], ctx), factor(input, s0[0], s1[0], base));
      }
      return evaluate(stops.at(-1)?.[1] ?? null, ctx);
    }
    case 'rgb':
    case 'rgba':
      return css([num(0) / 255, num(1) / 255, num(2) / 255, args.length > 3 ? num(3) : 1]);
    case '+':
      return nums().reduce((a, b) => a + b, 0);
    case '*':
      return nums().reduce((a, b) => a * b, 1);
    case '-':
      return args.length === 1 ? -num(0) : num(0) - num(1);
    case '/':
      return num(0) / num(1);
    case '%':
      return num(0) % num(1);
    case '^':
      return num(0) ** num(1);
    case 'min':
      return Math.min(...nums());
    case 'max':
      return Math.max(...nums());
    case 'abs':
      return Math.abs(num(0));
    case 'round':
      // Half away from zero, as MapLibre
      return Math.sign(num(0)) * Math.round(Math.abs(num(0)));
    case 'floor':
      return Math.floor(num(0));
    case 'ceil':
      return Math.ceil(num(0));
    case 'sqrt':
      return Math.sqrt(num(0));
    case 'ln':
      return Math.log(num(0));
    case 'log10':
      return Math.log10(num(0));
    case 'log2':
      return Math.log2(num(0));
    case 'pi':
      return Math.PI;
    case 'e':
      return Math.E;
    default:
      // Unsupported: no value (a filter drops the feature, a property takes its default)
      if (!warned.has(op)) {
        warned.add(op);
        console.warn(`isard: the style expression "${op}" is not supported`);
      }
      return null;
  }
}

// ── Filters ──

const LEGACY_COMPARISONS = new Set(['==', '!=', '<', '<=', '>', '>=']);

/** Whether a filter (not `all`, `any`, `none`) is in the legacy form: `["==", "key", value]`, `["in", "key", a, b]`,
 * `["has", "key"]`; a key and literal values, as MapLibre tells them from expressions */
function isLegacyLeaf(f: Json[]): boolean {
  const [op, a, ...rest] = f;
  if (typeof op !== 'string' || typeof a !== 'string' || rest.some(Array.isArray)) return false;
  if (LEGACY_COMPARISONS.has(op)) return f.length === 3;
  if (op === 'in') return f.length !== 3 || !Array.isArray(f[2]);
  return op === '!in' || op === 'has' || op === '!has';
}

function legacyFilter(f: Json[], ctx: Context): boolean {
  const [op, key, ...rest] = f;
  const v = get(ctx, key);
  switch (op) {
    case '==':
      return v === rest[0];
    case '!=':
      return v !== rest[0];
    case '<':
      return (v as number) < (rest[0] as number);
    case '<=':
      return (v as number) <= (rest[0] as number);
    case '>':
      return (v as number) > (rest[0] as number);
    case '>=':
      return (v as number) >= (rest[0] as number);
    case 'in':
      return rest.includes(v as Json);
    case '!in':
      return !rest.includes(v as Json);
    case 'has':
      return v !== undefined;
    case '!has':
      return v === undefined;
    default:
      return true;
  }
}

type Test = (ctx: Context) => boolean;

/** Compiled filters, by filter object */
const compiled = new WeakMap<Json[], Test>();

/** A filter as a function: `all`, `any`, `none` of their parts, each part legacy or an expression (they may mix) */
function compile(f: Json): Test {
  if (!Array.isArray(f)) return () => Boolean(f);
  const hit = compiled.get(f);
  if (hit) return hit;
  let test: Test;
  const op = f[0];
  if (op === 'all' || op === 'any' || op === 'none') {
    const parts = f.slice(1).map(compile);
    if (op === 'all') test = (ctx) => parts.every((p) => p(ctx));
    else if (op === 'any') test = (ctx) => parts.some((p) => p(ctx));
    else test = (ctx) => !parts.some((p) => p(ctx));
  } else if (isLegacyLeaf(f)) {
    test = (ctx) => legacyFilter(f, ctx);
  } else {
    test = (ctx) => Boolean(evaluate(f, ctx));
  }
  compiled.set(f, test);
  return test;
}

export function filter(f: Json | undefined, ctx: Context): boolean {
  if (f === undefined || f === null) return true;
  return compile(f)(ctx);
}

/** Whether a value depends on the feature (else only on the zoom, evaluated once per tile) */
export function isFeatureDependent(e: Json | undefined): boolean {
  if (e !== undefined && isFunction(e)) return e.property !== undefined;
  if (!Array.isArray(e)) return false;
  if (['get', 'has', '!has', 'geometry-type', 'properties', 'id'].includes(e[0] as string)) return true;
  return e.some((x) => isFeatureDependent(x));
}

/** A `{token}` text field (or an expression) to the text of a feature */
export function text(field: Json | undefined, ctx: Context): string {
  if (field === undefined || field === null) return '';
  if (typeof field === 'string') {
    return field.replace(/\{([^}]+)\}/g, (_, key: string) => {
      const v = ctx.properties[key];
      return v === undefined ? '' : String(v);
    });
  }
  const v = evaluate(field, ctx);
  if (Array.isArray(v)) {
    // A `format` value: its strings
    return v.filter((x) => typeof x === 'string').join('');
  }
  return v === undefined || v === null ? '' : String(v);
}

/** The font of a label from the first of its `text-font`: a family without its weight takes the regular one */
export function labelFont(font: string): string {
  return /Regular|Bold|Italic|Semibold|Light|Medium|Black/.test(font) ? font : `${font} Regular`;
}

/**
 * A value of the zoom only (an expression: `interpolate`, `step` of `["zoom"]`) as stops `[[zoom, value], …]` for the
 * engine, which interpolates them linearly: evaluated at its own stops and at the whole zooms between (an exponential
 * curve followed closely), a step kept sharp just before its threshold
 */
export function zoomStops(e: Json): [number, number][] {
  const zooms = new Set<number>();
  const walk = (x: Json) => {
    if (!Array.isArray(x)) return;
    const zoomInput = (input: Json | undefined) => Array.isArray(input) && input[0] === 'zoom';
    if ((x[0] === 'interpolate' || x[0] === 'interpolate-hcl' || x[0] === 'interpolate-lab') && zoomInput(x[2])) {
      for (let i = 3; i < x.length; i += 2) zooms.add(Number(x[i]));
    } else if (x[0] === 'step' && zoomInput(x[1])) {
      for (let i = 3; i < x.length; i += 2) {
        const t = Number(x[i]);
        zooms.add(t);
        zooms.add(t - 0.01);
      }
    }
    for (const y of x) walk(y);
  };
  walk(e);
  const finite = [...zooms].filter(Number.isFinite);
  if (finite.length === 0) finite.push(0);
  const [lo, hi] = [Math.min(...finite), Math.max(...finite)];
  for (let z = Math.ceil(lo); z <= Math.floor(hi); z += 1) finite.push(z);
  return [...new Set(finite)]
    .sort((a, b) => a - b)
    .map((zoom) => [zoom, Number(evaluate(e, { zoom, properties: {}, type: 'Unknown' }))]);
}
