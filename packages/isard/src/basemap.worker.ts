/**
 * Worker of the vector basemaps: fetches and decodes vector tiles (kept for the tiles that share them), draws their
 * fills, lines and circles with a MapLibre style into images (Canvas 2D, off the main thread) for the raster pool,
 * and lists the label candidates of the style's symbol layers for the page to shape and place.
 *
 * A raster tile of level z covers 256 CSS pixels at map zoom z - 1 (MapLibre's 512 px convention): it is drawn from
 * the vector tile of level z - 1 (or the deepest one the source has), with the style at that zoom.
 */
import { decode, type TileFeature, type TileLayer } from './mvt.ts';
import {
  type Context,
  color,
  context,
  css,
  evaluate,
  filter,
  isExpression,
  isFeatureDependent,
  type LabelCandidate,
  type StyleLayer,
  text,
} from './vectorstyle.ts';

export type { LabelCandidate };

export type BasemapRequest =
  | {
      type: 'init';
      /** Renderers by key: each draws its own layers of the same source */
      key: string;
      tiles: string;
      maxzoom: number;
      layers: StyleLayer[];
      /** The style's sprite, for the fill patterns (loaded once by the page) */
      sprite?: Sprite;
    }
  | { type: 'render'; key: string; id: number; z: number; x: number; y: number; size: number }
  | { type: 'labels'; key: string; id: number; z: number; x: number; y: number }
  | { type: 'abort'; id: number };

export type BasemapResponse =
  | { id: number; image: ImageBitmap }
  | { id: number; missing: true }
  | { id: number; error: string }
  | { id: number; labels: LabelCandidate[] };

interface Renderer {
  tiles: string;
  maxzoom: number;
  layers: StyleLayer[];
  sprite: Sprite | undefined;
  /** Fill patterns by icon name, cut from the sprite once */
  patterns: Map<string, OffscreenCanvas | undefined>;
}

export interface Sprite {
  image: ImageBitmap;
  icons: Record<string, { x: number; y: number; width: number; height: number; pixelRatio?: number }>;
}

const renderers = new Map<string, Renderer>();
/** Decoded tiles by URL, the most recent last */
const cache = new Map<string, Promise<Map<string, TileLayer> | undefined>>();
const CACHE = 96;
/** Requests in progress, and those of them aborted (both forgotten once answered) */
const active = new Set<number>();
const aborted = new Set<number>();

function tile(url: string): Promise<Map<string, TileLayer> | undefined> {
  const hit = cache.get(url);
  if (hit) {
    cache.delete(url);
    cache.set(url, hit);
    return hit;
  }
  const pending = (async () => {
    for (let attempt = 0; ; attempt += 1) {
      const response = await fetch(url);
      if (response.status === 404 || response.status === 204) return undefined;
      if (response.ok) {
        // Every layer: renderers of other subsets share the tile
        return decode(new Uint8Array(await response.arrayBuffer()));
      }
      if (attempt >= 2) throw new Error(`${url}: ${response.status}`);
      await new Promise((r) => setTimeout(r, 400 * (attempt + 1)));
    }
  })();
  pending.catch(() => cache.delete(url));
  cache.set(url, pending);
  while (cache.size > CACHE) {
    const oldest = cache.keys().next().value;
    if (oldest === undefined) break;
    cache.delete(oldest);
  }
  return pending;
}

/** The vector tile to draw a raster tile from, and where the raster tile lies in it (tile units) */
function source(r: Renderer, z: number, x: number, y: number) {
  const vz = Math.max(0, Math.min(z - 1, r.maxzoom));
  const d = z - vz;
  const n = 2 ** Math.max(0, d);
  const [vx, vy] = d >= 0 ? [Math.floor(x / n), Math.floor(y / n)] : [x, y];
  const url = r.tiles.replace('{z}', String(vz)).replace('{x}', String(vx)).replace('{y}', String(vy));
  // Fraction of the vector tile covered, and the offset in it
  const part = 1 / n;
  return { url, vz, vx, vy, part, ox: (x % n) * part, oy: (y % n) * part };
}

const value = (layer: StyleLayer, group: 'paint' | 'layout', key: string, ctx: Context, fallback: unknown) => {
  const v = layer[group]?.[key];
  if (v === undefined) return fallback;
  // An array that is not an expression is a literal value (`text-font`, `text-offset`, `line-dasharray`)
  const out = Array.isArray(v) && !isExpression(v) ? v : evaluate(v, ctx);
  return out === undefined || out === null ? fallback : out;
};

/** A color with an opacity, for the canvas */
function paint(c: unknown, opacity: number): string {
  const rgba = color(c) ?? [0, 0, 0, 1];
  return css([rgba[0], rgba[1], rgba[2], rgba[3] * opacity]);
}

function shown(layer: StyleLayer, zoom: number): boolean {
  if (layer.layout?.visibility === 'none') return false;
  return zoom >= (layer.minzoom ?? 0) && zoom < (layer.maxzoom ?? 24);
}

/** The image of a fill pattern, cut from the sprite on first use */
function patternImage(r: Renderer, name: string): OffscreenCanvas | undefined {
  if (r.patterns.has(name)) return r.patterns.get(name);
  const icon = r.sprite?.icons[name];
  let image: OffscreenCanvas | undefined;
  if (r.sprite && icon && icon.width > 0 && icon.height > 0) {
    image = new OffscreenCanvas(icon.width, icon.height);
    image
      .getContext('2d')
      ?.drawImage(r.sprite.image, icon.x, icon.y, icon.width, icon.height, 0, 0, icon.width, icon.height);
  }
  r.patterns.set(name, image);
  return image;
}

async function render(r: Renderer, z: number, x: number, y: number, size: number, id: number) {
  const s = source(r, z, x, y);
  const layers = await tile(s.url);
  if (aborted.has(id)) return undefined;
  const sprite = r.sprite;
  const zoom = Math.max(0, z - 1);
  const canvas = new OffscreenCanvas(size, size);
  // A canvas on the CPU: its pixels are read back here, in the worker, not on the main thread at the upload
  const g = canvas.getContext('2d', { willReadFrequently: true });
  if (!g) throw new Error('no 2D context');
  // CSS pixels to canvas pixels: the tile is 256 CSS pixels across at its zoom
  const px = size / 256;
  let drawn = false;
  for (const layer of r.layers) {
    if (!shown(layer, zoom)) continue;
    const zctx: Context = { zoom, properties: {}, type: 'Unknown' };
    if (layer.type === 'background') {
      g.fillStyle = paint(
        value(layer, 'paint', 'background-color', zctx, '#000'),
        Number(value(layer, 'paint', 'background-opacity', zctx, 1)),
      );
      g.fillRect(0, 0, size, size);
      drawn = true;
      continue;
    }
    const data = layers?.get(layer['source-layer'] ?? '');
    if (!data) continue;
    // Tile units to canvas pixels
    const k = size / (data.extent * s.part);
    const [ox, oy] = [s.ox * data.extent, s.oy * data.extent];
    const path = (f: TileFeature, close: boolean) => {
      g.beginPath();
      for (const line of f.geometry) {
        for (let i = 0; i + 1 < line.length; i += 2) {
          const [lx, ly] = [((line[i] ?? 0) - ox) * k, ((line[i + 1] ?? 0) - oy) * k];
          if (i === 0) g.moveTo(lx, ly);
          else g.lineTo(lx, ly);
        }
        if (close) g.closePath();
      }
    };
    // Values that only depend on the zoom are evaluated once for the layer
    const dependent = new Map<string, boolean>();
    const get = (group: 'paint' | 'layout', key: string, ctx: Context, fallback: unknown) => {
      let d = dependent.get(key);
      if (d === undefined) {
        d = isFeatureDependent(layer[group]?.[key]);
        dependent.set(key, d);
      }
      return value(layer, group, key, d ? ctx : zctx, fallback);
    };
    for (const f of data.features) {
      const ctx = context(f, zoom);
      if (!filter(layer.filter, ctx)) continue;
      if (layer.type === 'fill' && f.type === 3) {
        const opacity = Number(get('paint', 'fill-opacity', ctx, 1));
        const pattern = get('paint', 'fill-pattern', ctx, undefined);
        path(f, true);
        const icon = typeof pattern === 'string' ? sprite?.icons[pattern] : undefined;
        const image = icon && patternImage(r, pattern as string);
        if (icon && image) {
          const p = g.createPattern(image, 'repeat');
          if (p) {
            p.setTransform(new DOMMatrix().scale(px / (icon.pixelRatio ?? 1)));
            g.globalAlpha = opacity;
            g.fillStyle = p;
            g.fill('evenodd');
            g.globalAlpha = 1;
          }
        } else {
          g.fillStyle = paint(get('paint', 'fill-color', ctx, '#000'), opacity);
          g.fill('evenodd');
          const outline = get('paint', 'fill-outline-color', ctx, undefined);
          if (outline !== undefined) {
            g.strokeStyle = paint(outline, opacity);
            g.lineWidth = px;
            g.setLineDash([]);
            g.stroke();
          }
        }
        drawn = true;
      } else if (layer.type === 'line' && f.type !== 1) {
        const width = Number(get('paint', 'line-width', ctx, 1));
        if (width <= 0) continue;
        g.strokeStyle = paint(get('paint', 'line-color', ctx, '#000'), Number(get('paint', 'line-opacity', ctx, 1)));
        g.lineWidth = width * px;
        g.lineCap = String(get('layout', 'line-cap', ctx, 'butt')) as CanvasLineCap;
        g.lineJoin = String(get('layout', 'line-join', ctx, 'miter')) as CanvasLineJoin;
        const dash = get('paint', 'line-dasharray', ctx, undefined);
        g.setLineDash(Array.isArray(dash) ? dash.map((d) => Number(d) * width * px) : []);
        path(f, f.type === 3);
        g.stroke();
        drawn = true;
      } else if (layer.type === 'circle' && f.type === 1) {
        const radius = Number(get('paint', 'circle-radius', ctx, 5)) * px;
        g.fillStyle = paint(get('paint', 'circle-color', ctx, '#000'), Number(get('paint', 'circle-opacity', ctx, 1)));
        const stroke = Number(get('paint', 'circle-stroke-width', ctx, 0)) * px;
        g.strokeStyle = paint(
          get('paint', 'circle-stroke-color', ctx, '#000'),
          Number(get('paint', 'circle-stroke-opacity', ctx, 1)),
        );
        g.lineWidth = stroke;
        for (const point of f.geometry) {
          for (let i = 0; i + 1 < point.length; i += 2) {
            g.beginPath();
            g.arc(((point[i] ?? 0) - ox) * k, ((point[i + 1] ?? 0) - oy) * k, radius, 0, 2 * Math.PI);
            g.fill();
            if (stroke > 0) g.stroke();
          }
        }
        drawn = true;
      }
    }
  }
  if (!drawn && !layers) return null;
  return createImageBitmap(g.getImageData(0, 0, size, size), { premultiplyAlpha: 'none' });
}

/** Longitude and latitude of a point of tile (z, x, y) in tile units */
function lonLat(z: number, x: number, y: number, extent: number, px: number, py: number): [number, number] {
  const n = 2 ** z;
  const mx = (x + px / extent) / n;
  const my = (y + py / extent) / n;
  return [mx * 360 - 180, (Math.atan(Math.sinh(Math.PI * (1 - 2 * my))) * 180) / Math.PI];
}

async function labels(r: Renderer, z: number, x: number, y: number, id: number): Promise<LabelCandidate[]> {
  const url = r.tiles.replace('{z}', String(z)).replace('{x}', String(x)).replace('{y}', String(y));
  const layers = await tile(url);
  if (!layers || aborted.has(id)) return [];
  const out: LabelCandidate[] = [];
  r.layers.forEach((layer, index) => {
    if (layer.type !== 'symbol' || !shown(layer, z) || layer.layout?.['text-field'] === undefined) return;
    const data = layers.get(layer['source-layer'] ?? '');
    if (!data) return;
    const placement = layer.layout?.['symbol-placement'] === 'line' ? 'line' : 'point';
    for (const f of data.features) {
      const ctx = context(f, z);
      if (!filter(layer.filter, ctx)) continue;
      let t = text(layer.layout?.['text-field'], ctx).trim();
      if (!t) continue;
      const transform = value(layer, 'layout', 'text-transform', ctx, 'none');
      if (transform === 'uppercase') t = t.toUpperCase();
      else if (transform === 'lowercase') t = t.toLowerCase();
      const fonts = value(layer, 'layout', 'text-font', ctx, ['Source Sans Pro Regular']);
      let font = Array.isArray(fonts) && typeof fonts[0] === 'string' ? fonts[0] : 'Source Sans Pro Regular';
      // A family without its weight: the regular one
      if (!/Regular|Bold|Italic|Semibold|Light/.test(font)) font = `${font} Regular`;
      const offset = value(layer, 'layout', 'text-offset', ctx, [0, 0]);
      const base = {
        layer: index,
        text: t,
        font,
        // Zoom stops are kept for the engine; a value of the feature evaluated here
        size: isFeatureDependent(layer.layout?.['text-size'])
          ? Number(value(layer, 'layout', 'text-size', ctx, 16))
          : (layer.layout?.['text-size'] ?? 16),
        color: String(value(layer, 'paint', 'text-color', ctx, '#000')),
        haloColor: String(value(layer, 'paint', 'text-halo-color', ctx, 'rgba(0,0,0,0)')),
        haloWidth: Number(value(layer, 'paint', 'text-halo-width', ctx, 0)),
        anchor: String(value(layer, 'layout', 'text-anchor', ctx, 'center')),
        offset: (Array.isArray(offset) ? [Number(offset[0] ?? 0), Number(offset[1] ?? 0)] : [0, 0]) as [number, number],
        maxWidth: Number(value(layer, 'layout', 'text-max-width', ctx, 10)),
        lineHeight: Number(value(layer, 'layout', 'text-line-height', ctx, 1.2)),
        overlap: Boolean(value(layer, 'layout', 'text-allow-overlap', ctx, false)),
      };
      if (f.type === 1) {
        for (const point of f.geometry) {
          for (let i = 0; i + 1 < point.length; i += 2) {
            const [px, py] = [point[i] ?? 0, point[i + 1] ?? 0];
            // Points in the buffer belong to the neighbouring tile
            if (px < 0 || py < 0 || px >= data.extent || py >= data.extent) continue;
            const [lon, lat] = lonLat(z, x, y, data.extent, px, py);
            out.push({ ...base, lon, lat });
          }
        }
      } else {
        // The middle of the longest line inside the tile, along it
        let best: number[] | undefined;
        let bestLength = 0;
        for (const line of f.geometry) {
          let length = 0;
          for (let i = 2; i + 1 < line.length; i += 2) {
            length += Math.hypot((line[i] ?? 0) - (line[i - 2] ?? 0), (line[i + 1] ?? 0) - (line[i - 1] ?? 0));
          }
          if (length > bestLength) {
            bestLength = length;
            best = line;
          }
        }
        if (!best || bestLength < data.extent / 32) continue;
        let rest = bestLength / 2;
        for (let i = 2; i + 1 < best.length; i += 2) {
          const [ax, ay, bx, by] = [best[i - 2] ?? 0, best[i - 1] ?? 0, best[i] ?? 0, best[i + 1] ?? 0];
          const d = Math.hypot(bx - ax, by - ay);
          if (d >= rest && d > 0) {
            const [px, py] = [ax + ((bx - ax) * rest) / d, ay + ((by - ay) * rest) / d];
            if (px < 0 || py < 0 || px >= data.extent || py >= data.extent) break;
            const [lon, lat] = lonLat(z, x, y, data.extent, px, py);
            const [lon1, lat1] = lonLat(z, x, y, data.extent, bx, by);
            const [lon0, lat0] = lonLat(z, x, y, data.extent, ax, ay);
            out.push({ ...base, lon, lat, ...(placement === 'line' ? { along: [lon1 - lon0, lat1 - lat0] } : {}) });
            break;
          }
          rest -= d;
        }
      }
    }
  });
  return out;
}

const post = (message: BasemapResponse, transfer: Transferable[] = []) =>
  (self as unknown as Worker).postMessage(message, transfer);

self.onmessage = async ({ data }: MessageEvent<BasemapRequest>) => {
  if (data.type === 'init') {
    renderers.set(data.key, {
      tiles: data.tiles,
      maxzoom: data.maxzoom,
      layers: data.layers,
      sprite: data.sprite,
      patterns: new Map(),
    });
    return;
  }
  if (data.type === 'abort') {
    // An abort that arrives after the answer is ignored
    if (active.has(data.id)) aborted.add(data.id);
    return;
  }
  active.add(data.id);
  const r = renderers.get(data.key);
  try {
    if (!r) throw new Error(`no renderer ${data.key}`);
    if (data.type === 'render') {
      const image = await render(r, data.z, data.x, data.y, data.size, data.id);
      if (image) post({ id: data.id, image }, [image]);
      else post({ id: data.id, missing: true });
    } else {
      post({ id: data.id, labels: await labels(r, data.z, data.x, data.y, data.id) });
    }
  } catch (error) {
    post({ id: data.id, error: String(error) });
  } finally {
    active.delete(data.id);
    aborted.delete(data.id);
  }
};
