/**
 * The parts of the MapLibre style specification a vector basemap needs, evaluated on the page: filters (legacy
 * `["==", "key", value]` forms and expressions), property values given as constants, legacy functions (`{ stops }`
 * of zoom or of a property: exponential, interval, categorical, identity) or expressions (`get`, `has`, `concat`,
 * `interpolate`, `step`, `match`, `case`, `coalesce`, comparisons, `all`, `any`, `!`, `in`, `zoom`, `to-number`,
 * `to-string`, `upcase`, `downcase`, `literal`), colors interpolated in RGBA, `{token}` text fields.
 */
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
  sources: Record<string, { type: string; tiles?: string[]; url?: string; maxzoom?: number; minzoom?: number }>;
  layers: StyleLayer[];
  glyphs?: string;
  sprite?: string;
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
  if (typeof value !== 'string') return undefined;
  const c = value.trim().toLowerCase();
  if (c.startsWith('#')) {
    const hex = c.slice(1);
    const full = hex.length <= 4 ? [...hex].map((h) => h + h).join('') : hex;
    const v = (i: number) => Number.parseInt(full.slice(i, i + 2), 16) / 255;
    return [v(0), v(2), v(4), full.length >= 8 ? v(6) : 1];
  }
  const match = /^(rgba?|hsla?)\(([^)]*)\)$/.exec(c);
  if (match?.[1] && match[2] !== undefined) {
    const parts = match[2]
      .split(/[\s,/]+/)
      .filter(Boolean)
      .map((p) => Number.parseFloat(p));
    const a = parts[3] ?? 1;
    if (match[1].startsWith('rgb')) return [(parts[0] ?? 0) / 255, (parts[1] ?? 0) / 255, (parts[2] ?? 0) / 255, a];
    const [h, s, l] = [((parts[0] ?? 0) % 360) / 360, (parts[1] ?? 0) / 100, (parts[2] ?? 0) / 100];
    const q = l < 0.5 ? l * (1 + s) : l + s - l * s;
    const p = 2 * l - q;
    const hue = (t: number) => {
      const u = (t + 1) % 1;
      if (u < 1 / 6) return p + (q - p) * 6 * u;
      if (u < 1 / 2) return q;
      if (u < 2 / 3) return p + (q - p) * (2 / 3 - u) * 6;
      return p;
    };
    return [hue(h + 1 / 3), hue(h), hue(h - 1 / 3), a];
  }
  const named: Record<string, [number, number, number, number]> = {
    black: [0, 0, 0, 1],
    white: [1, 1, 1, 1],
    transparent: [0, 0, 0, 0],
    red: [1, 0, 0, 1],
    blue: [0, 0, 1, 1],
    green: [0, 0.5, 0, 1],
    grey: [0.5, 0.5, 0.5, 1],
    gray: [0.5, 0.5, 0.5, 1],
  };
  return named[c];
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

export function evaluate(e: Json, ctx: Context): unknown {
  if (isFunction(e)) return legacy(e, ctx);
  if (!Array.isArray(e)) return e;
  const [op, ...args] = e;
  if (typeof op !== 'string') return e;
  const ev = (i: number) => evaluate(args[i] ?? null, ctx);
  switch (op) {
    case 'literal':
      return args[0];
    case 'zoom':
      return ctx.zoom;
    case 'get':
      return get(ctx, ev(0));
    case 'has':
      return get(ctx, ev(0)) !== undefined;
    case 'geometry-type':
      return ctx.type;
    case 'concat':
      return args
        .map((_, i) => {
          const v = ev(i);
          return v === undefined || v === null ? '' : String(v);
        })
        .join('');
    case 'to-number': {
      for (let i = 0; i < args.length; i += 1) {
        const n = Number(ev(i));
        if (!Number.isNaN(n)) return n;
      }
      return 0;
    }
    case 'to-string': {
      const v = ev(0);
      return v === undefined || v === null ? '' : String(v);
    }
    case 'upcase':
      return String(ev(0) ?? '').toUpperCase();
    case 'downcase':
      return String(ev(0) ?? '').toLowerCase();
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
      const input = Number(ev(0));
      let out = ev(1);
      for (let i = 2; i + 1 < args.length; i += 2) {
        if (input >= Number(args[i])) out = ev(i + 1);
        else break;
      }
      return out;
    }
    case 'interpolate': {
      const kind = args[0];
      const input = Number(ev(1));
      const base = Array.isArray(kind) && kind[0] === 'exponential' ? Number(kind[1]) : 1;
      const stops: [number, Json][] = [];
      for (let i = 2; i + 1 < args.length; i += 2) stops.push([Number(args[i]), args[i + 1] ?? null]);
      const first = stops[0];
      if (!first) return undefined;
      if (input <= first[0]) return evaluate(first[1], ctx);
      for (let i = 1; i < stops.length; i += 1) {
        const [s0, s1] = [stops[i - 1], stops[i]];
        if (s0 && s1 && input < s1[0])
          return mix(evaluate(s0[1], ctx), evaluate(s1[1], ctx), factor(input, s0[0], s1[0], base));
      }
      return evaluate(stops.at(-1)?.[1] ?? null, ctx);
    }
    default:
      return e;
  }
}

// ── Filters ──

const LEGACY_COMPARISONS = new Set(['==', '!=', '<', '<=', '>', '>=']);

/** Whether a filter is in the legacy form: `["==", "key", value]`, `["in", "key", a, b]`, `["has", "key"]` */
function isLegacy(f: Json): boolean {
  if (!Array.isArray(f) || typeof f[0] !== 'string') return false;
  const [op, a] = f;
  if (op === 'all' || op === 'any' || op === 'none') return f.slice(1).some(isLegacy) || f.length === 1;
  if (LEGACY_COMPARISONS.has(op) || op === 'in' || op === '!in' || op === 'has' || op === '!has') {
    return typeof a === 'string';
  }
  return false;
}

function legacyFilter(f: Json, ctx: Context): boolean {
  if (!Array.isArray(f)) return true;
  const [op, key, ...rest] = f;
  const v = get(ctx, key);
  switch (op) {
    case 'all':
      return f.slice(1).every((g) => legacyFilter(g, ctx));
    case 'any':
      return f.slice(1).some((g) => legacyFilter(g, ctx));
    case 'none':
      return !f.slice(1).some((g) => legacyFilter(g, ctx));
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

export function filter(f: Json | undefined, ctx: Context): boolean {
  if (f === undefined || f === null) return true;
  if (isLegacy(f)) return legacyFilter(f, ctx);
  return Boolean(evaluate(f, ctx));
}

/** Whether a value depends on the feature (else only on the zoom, evaluated once per tile) */
export function isFeatureDependent(e: Json | undefined): boolean {
  if (e !== undefined && isFunction(e)) return e.property !== undefined;
  if (!Array.isArray(e)) return false;
  if (e[0] === 'get' || e[0] === 'has' || e[0] === 'geometry-type' || e[0] === 'properties') return true;
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
