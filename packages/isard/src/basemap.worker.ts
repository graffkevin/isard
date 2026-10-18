/**
 * Worker of the vector basemaps: fetches and decodes vector tiles (kept for the tiles that share them), draws their
 * fills, lines and circles with a MapLibre style into images (Canvas 2D, off the main thread) for the raster pool,
 * with the style's raster layers under them, and lists the label candidates of the style's symbol layers for the page
 * to shape and place. Extrusions are drawn flat, as fills. It also decodes the altitudes of PNG relief tiles and
 * draws the rasters reprojected from other grids (planned by the page), for the map's other layers.
 *
 * A raster tile of level z covers 256 CSS pixels at map zoom z - 1 (MapLibre's 512 px convention): it is drawn from
 * the vector tile of level z - 1 (or the deepest one the source has), with the style at that zoom.
 */
import { latitude, longitude } from './geo.ts';
import { Lru } from './lru.ts';
import { decode, type TileFeature, type TileLayer } from './mvt.ts';
import { type Job, resampler } from './resample.ts';
import { fetchHeights, type PngEncoding, type Surface } from './terrain.ts';
import {
  type Context,
  color,
  context,
  css,
  evaluate,
  filter,
  isExpression,
  isFeatureDependent,
  type Json,
  type LabelCandidate,
  labelFont,
  type RasterSource,
  type StyleLayer,
  text,
  zoomStops,
} from './vectorstyle.ts';

export type { LabelCandidate, RasterSource };

/** Fetch priority of a request (`fetch`'s `priority`, a hint to the browser) */
export type Priority = 'high' | 'low' | 'auto';

/** A tile's image or labels, asked of a worker; or a tile of another layer decoded: the altitudes of a PNG relief
 * tile, a Mercator tile reprojected from the source images of its plan (keeping `cached` of them) */
export type BasemapWork =
  | { type: 'render'; key: string; z: number; x: number; y: number; size: number; priority?: Priority }
  | { type: 'labels'; key: string; z: number; x: number; y: number; priority?: Priority }
  | { type: 'heights'; url: string; encoding: PngEncoding; floor?: number }
  | { type: 'reproject'; job: Job; cached: number };

export type BasemapRequest =
  | {
      type: 'init';
      /** Renderers by key: each draws its own layers of the same source */
      key: string;
      tiles: string;
      maxzoom: number;
      layers: StyleLayer[];
      /** The style's raster sources, by name, for its raster layers */
      rasters?: Record<string, RasterSource>;
      /** The style's sprite, for the fill patterns (loaded once by the page) */
      sprite?: Sprite;
      /** Workers of the pool: each keeps its share of the decoded tiles */
      workers?: number;
    }
  | (BasemapWork & { id: number })
  | { type: 'abort'; id: number }
  /** A renderer no longer used: forgotten */
  | { type: 'drop'; key: string };

export type BasemapResponse =
  | { id: number; image: ImageBitmap }
  | { id: number; missing: true }
  | { id: number; error: string }
  | { id: number; labels: LabelCandidate[] }
  | { id: number; heights: Float32Array };

interface Renderer {
  tiles: string;
  maxzoom: number;
  layers: StyleLayer[];
  rasters: Record<string, RasterSource>;
  sprite: Sprite | undefined;
  /** Fill patterns by icon name, cut from the sprite once */
  patterns: Map<string, OffscreenCanvas | undefined>;
}

export interface Sprite {
  image: ImageBitmap;
  icons: Record<string, { x: number; y: number; width: number; height: number; pixelRatio?: number }>;
}

const renderers = new Map<string, Renderer>();
/** Decoded tiles kept by the pool, shared among its workers (a vector tile always goes to the same worker): some
 * 2 MB each */
const CACHE = 96;
/** Decoded tiles by URL; this worker's share of them, set by the renderers */
const cache = new Lru<string, Promise<Map<string, TileLayer> | undefined>>(CACHE);

/** The decoded tiles one worker of `workers` keeps */
export function cacheShare(workers: number): number {
  return Math.max(16, Math.ceil(CACHE / Math.max(1, workers)));
}
/** Requests in progress, and those of them aborted (both forgotten once answered) */
const active = new Set<number>();
const aborted = new Set<number>();
/** The signals of the relief and reprojection requests in progress, aborted with them */
const controllers = new Map<number, AbortController>();

/** Where this worker reads the colors of the relief tiles back */
const heightsSurface: Surface = {};

/** The reprojected rasters' source images this worker keeps, shared by every source */
const reprojection = resampler();

/** Images of the raster sources by URL. An image dropped is freed once settled, after the renders that awaited it have
 * drawn it (they draw without awaiting again) */
const images = new Lru<string, Promise<ImageBitmap | undefined>>(64, (evicted) =>
  evicted.then(
    (bitmap) => setTimeout(() => bitmap?.close(), 0),
    () => undefined,
  ),
);

/** An image of a raster source (shared by every renderer); undefined where the source has none */
function image(url: string, priority: Priority = 'auto'): Promise<ImageBitmap | undefined> {
  const hit = images.get(url);
  if (hit) return hit;
  const pending = (async () => {
    const init: PriorityInit = { priority };
    const response = await fetch(url, init);
    if (response.status === 404 || response.status === 204) return undefined;
    if (!response.ok) throw new Error(`${url}: ${response.status}`);
    return createImageBitmap(await response.blob());
  })();
  pending.catch(() => images.delete(url));
  images.set(url, pending);
  return pending;
}

/** `RequestInit` with the fetch priority, which the TypeScript DOM library may not know yet */
type PriorityInit = RequestInit & { priority?: Priority };

/** A vector tile, decoded (shared by every renderer of its URL); the first request's priority fetches it */
function tile(url: string, priority: Priority = 'auto'): Promise<Map<string, TileLayer> | undefined> {
  const hit = cache.get(url);
  if (hit) return hit;
  const pending = (async () => {
    for (let attempt = 0; ; attempt += 1) {
      const init: PriorityInit = { priority };
      const response = await fetch(url, init);
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

/** Canvases free for the next tiles, by size: one per tile being drawn at most */
const canvases = new Map<number, OffscreenCanvas[]>();

/** A blank canvas of `size` pixels and its context, from the free ones if any */
function takeCanvas(size: number): { canvas: OffscreenCanvas; g: OffscreenCanvasRenderingContext2D } {
  const canvas = canvases.get(size)?.pop() ?? new OffscreenCanvas(size, size);
  // A canvas on the CPU: its pixels are read back here, in the worker, not on the main thread at the upload
  const g = canvas.getContext('2d', { willReadFrequently: true });
  if (!g) throw new Error('no 2D context');
  // The state a former tile left
  g.setTransform(1, 0, 0, 1, 0, 0);
  g.globalAlpha = 1;
  g.setLineDash([]);
  g.clearRect(0, 0, size, size);
  return { canvas, g };
}

function giveCanvas(canvas: OffscreenCanvas): void {
  const free = canvases.get(canvas.width) ?? [];
  canvases.set(canvas.width, free);
  if (free.length < 4) free.push(canvas);
}

/**
 * The image of a drawn canvas, its alpha straight (the engine blends straight alpha). An opaque tile (a background
 * of full opacity, as a basemap's) is moved out of the canvas as it is, premultiplied and straight being the same
 * there; another one is read back unpremultiplied, copied
 */
export function tileImage(
  canvas: OffscreenCanvas,
  g: OffscreenCanvasRenderingContext2D,
  opaque: boolean,
): Promise<ImageBitmap> {
  if (opaque) return Promise.resolve(canvas.transferToImageBitmap());
  return createImageBitmap(g.getImageData(0, 0, canvas.width, canvas.height), { premultiplyAlpha: 'none' });
}

async function render(r: Renderer, z: number, x: number, y: number, size: number, id: number, priority?: Priority) {
  const s = source(r, z, x, y);
  const layers = await tile(s.url, priority);
  if (aborted.has(id)) return undefined;
  const { canvas, g } = takeCanvas(size);
  try {
    return await draw(r, s, layers, canvas, g, z, x, y, size, id, priority);
  } finally {
    giveCanvas(canvas);
  }
}

async function draw(
  r: Renderer,
  s: ReturnType<typeof source>,
  layers: Map<string, TileLayer> | undefined,
  canvas: OffscreenCanvas,
  g: OffscreenCanvasRenderingContext2D,
  z: number,
  x: number,
  y: number,
  size: number,
  id: number,
  priority?: Priority,
) {
  const sprite = r.sprite;
  const zoom = Math.max(0, z - 1);
  // CSS pixels to canvas pixels: the tile is 256 CSS pixels across at its zoom
  const px = size / 256;
  let drawn = false;
  /** A background of full opacity covers the tile: every pixel stays opaque */
  let opaque = false;
  for (const layer of r.layers) {
    if (!shown(layer, zoom)) continue;
    const zctx: Context = { zoom, properties: {}, type: 'Unknown' };
    if (layer.type === 'background') {
      const fill = value(layer, 'paint', 'background-color', zctx, '#000');
      const opacity = Number(value(layer, 'paint', 'background-opacity', zctx, 1));
      g.fillStyle = paint(fill, opacity);
      g.fillRect(0, 0, size, size);
      if ((color(fill) ?? [0, 0, 0, 1])[3] * opacity >= 1) opaque = true;
      drawn = true;
      continue;
    }
    if (layer.type === 'raster') {
      const source = layer.source === undefined ? undefined : r.rasters[layer.source];
      if (!source) continue;
      // The canvas covers the Web Mercator tile (z, x, y): a source tile of 512 px covers it from a level coarser
      const rz = Math.max(0, Math.min(z - (source.tileSize >= 512 ? 1 : 0), source.maxzoom));
      const n = 2 ** (z - rz);
      const url = source.tiles
        .replace('{z}', String(rz))
        .replace('{x}', String(Math.floor(x / n)))
        .replace('{y}', String(Math.floor(y / n)));
      const picture = await image(url, priority).catch(() => undefined);
      if (aborted.has(id)) return undefined;
      if (!picture) continue;
      const [w, h] = [picture.width / n, picture.height / n];
      g.globalAlpha = Number(value(layer, 'paint', 'raster-opacity', zctx, 1));
      g.drawImage(picture, (x % n) * w, (y % n) * h, w, h, 0, 0, size, size);
      g.globalAlpha = 1;
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
      if (layer.type === 'fill-extrusion' && f.type === 3) {
        // Flat, seen from above: its color
        path(f, true);
        g.fillStyle = paint(
          get('paint', 'fill-extrusion-color', ctx, '#000'),
          Number(get('paint', 'fill-extrusion-opacity', ctx, 1)),
        );
        g.fill('evenodd');
        drawn = true;
      } else if (layer.type === 'fill' && f.type === 3) {
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
        // A gap: two lines on either side of it, drawn as one wide line (the casing of a road, its inside drawn over)
        const gap = Number(get('paint', 'line-gap-width', ctx, 0));
        g.strokeStyle = paint(get('paint', 'line-color', ctx, '#000'), Number(get('paint', 'line-opacity', ctx, 1)));
        g.lineWidth = (gap > 0 ? gap + 2 * width : width) * px;
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
  return tileImage(canvas, g, opaque);
}

/** A text written over its icon (a road shield's number: no halo, centered on it, no offset): icons are not drawn, and
 * without its icon the text would float unreadable; such a layer is left out */
function onIcon(layer: StyleLayer): boolean {
  const { layout = {}, paint = {} } = layer;
  return (
    layout['icon-image'] !== undefined &&
    paint['text-halo-width'] === undefined &&
    paint['text-halo-color'] === undefined &&
    layout['text-offset'] === undefined &&
    (layout['text-anchor'] ?? 'center') === 'center'
  );
}

/** Longitude and latitude of a point of tile (z, x, y) in tile units */
function lonLat(z: number, x: number, y: number, extent: number, px: number, py: number): [number, number] {
  const n = 2 ** z;
  return [longitude((x + px / extent) / n), latitude((y + py / extent) / n)];
}

async function labels(
  r: Renderer,
  z: number,
  x: number,
  y: number,
  id: number,
  priority?: Priority,
): Promise<LabelCandidate[]> {
  const url = r.tiles.replace('{z}', String(z)).replace('{x}', String(x)).replace('{y}', String(y));
  const layers = await tile(url, priority);
  if (!layers || aborted.has(id)) return [];
  const out: LabelCandidate[] = [];
  // The text size of a layer when it depends on the zoom only: legacy stops as they are, an expression as stops
  const sizes = new Map<StyleLayer, Json>();
  const sizeOf = (layer: StyleLayer): Json => {
    let s = sizes.get(layer);
    if (s === undefined) {
      const v = layer.layout?.['text-size'] ?? 16;
      s = isExpression(v) ? zoomStops(v) : v;
      sizes.set(layer, s);
    }
    return s;
  };
  r.layers.forEach((layer, index) => {
    if (layer.type !== 'symbol' || !shown(layer, z) || layer.layout?.['text-field'] === undefined) return;
    if (onIcon(layer)) return;
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
      const fonts = value(layer, 'layout', 'text-font', ctx, ['Noto Sans Regular']);
      const font = labelFont(Array.isArray(fonts) && typeof fonts[0] === 'string' ? fonts[0] : 'Noto Sans Regular');
      const offset = value(layer, 'layout', 'text-offset', ctx, [0, 0]);
      const base = {
        layer: index,
        text: t,
        font,
        // Zoom stops are kept for the engine; a value of the feature evaluated here
        size: isFeatureDependent(layer.layout?.['text-size'])
          ? Number(value(layer, 'layout', 'text-size', ctx, 16))
          : sizeOf(layer),
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
    if (data.workers !== undefined) cache.capacity = cacheShare(data.workers);
    renderers.set(data.key, {
      tiles: data.tiles,
      maxzoom: data.maxzoom,
      layers: data.layers,
      rasters: data.rasters ?? {},
      sprite: data.sprite,
      patterns: new Map(),
    });
    return;
  }
  if (data.type === 'drop') {
    // Its copy of the sprite
    renderers.get(data.key)?.sprite?.image.close();
    renderers.delete(data.key);
    return;
  }
  if (data.type === 'abort') {
    // An abort that arrives after the answer is ignored
    if (active.has(data.id)) aborted.add(data.id);
    controllers.get(data.id)?.abort();
    return;
  }
  if (data.type === 'heights' || data.type === 'reproject') {
    await decodeTile(data);
    return;
  }
  active.add(data.id);
  const r = renderers.get(data.key);
  try {
    if (!r) throw new Error(`no renderer ${data.key}`);
    if (data.type === 'render') {
      const image = await render(r, data.z, data.x, data.y, data.size, data.id, data.priority);
      if (image) post({ id: data.id, image }, [image]);
      else post({ id: data.id, missing: true });
    } else {
      post({ id: data.id, labels: await labels(r, data.z, data.x, data.y, data.id, data.priority) });
    }
  } catch (error) {
    post({ id: data.id, error: String(error) });
  } finally {
    active.delete(data.id);
    aborted.delete(data.id);
  }
};

/** A tile of another layer: the altitudes of a relief tile, or a reprojected raster tile */
async function decodeTile(data: Extract<BasemapWork, { type: 'heights' | 'reproject' }> & { id: number }) {
  const controller = new AbortController();
  controllers.set(data.id, controller);
  try {
    if (data.type === 'heights') {
      const heights = await fetchHeights(data.url, data.encoding, data.floor, controller.signal, heightsSurface);
      if (heights) post({ id: data.id, heights }, [heights.buffer]);
      else post({ id: data.id, missing: true });
    } else {
      reprojection.cache.capacity = data.cached;
      const image = await reprojection.draw(data.job, controller.signal);
      if (image) post({ id: data.id, image }, [image]);
      else post({ id: data.id, missing: true });
    }
  } catch (error) {
    post({ id: data.id, error: String(error) });
  } finally {
    controllers.delete(data.id);
  }
}
