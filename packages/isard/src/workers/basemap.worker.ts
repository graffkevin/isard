/**
 * Worker of the vector basemaps: fetches and decodes vector tiles (kept for the tiles that share them), draws their
 * fills, lines and circles with a MapLibre style into images (Canvas 2D, off the main thread) for the raster pool,
 * with the style's raster layers under them, and lists the label candidates of the style's symbol layers (their texts
 * and icons) for the page to shape and place. Extrusions are drawn flat, as fills. It also decodes the altitudes of PNG relief tiles and
 * draws the rasters reprojected from other grids (planned by the page), for the map's other layers.
 *
 * A raster tile of level z covers 256 CSS pixels at map zoom z - 1 (MapLibre's 512 px convention): it is drawn from
 * the vector tile of level z - 1 (or the deepest one the source has), with the style at that zoom.
 */

import { decode, type TileFeature, type TileLayer } from '../basemap/mvt.ts';
import { pmtilesTile } from '../basemap/pmtiles.ts';
import { type Job, resampler } from '../basemap/resample.ts';
import { fetchHeights, type PngEncoding, type Surface } from '../basemap/terrain.ts';
import {
  type CandidateIcon,
  type CandidateOverlap,
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
} from '../basemap/vectorstyle.ts';
import { type HeaderRules, setHeaderRules, withHeaders } from '../engine/requests.ts';
import { latitude, longitude } from '../util/geo.ts';
import { Lru } from '../util/lru.ts';

export type { LabelCandidate, RasterSource };

/** Fetch priority of a request (`fetch`'s `priority`, a hint to the browser) */
export type Priority = 'high' | 'low' | 'auto';

/** A tile's image or labels, asked of a worker; or a tile of another layer decoded: the altitudes of a PNG relief
 * tile, a Mercator tile reprojected from the source images of its plan (keeping `cached` of them) */
export type BasemapWork =
  | { type: 'render'; key: string; z: number; x: number; y: number; size: number; priority?: Priority }
  /** `zoom`: the zoom the style is evaluated at, past the source's deepest level (`z` by default) */
  | { type: 'labels'; key: string; z: number; x: number; y: number; zoom?: number; priority?: Priority }
  | { type: 'heights'; url: string; encoding: PngEncoding; floor?: number }
  | { type: 'reproject'; job: Job; cached: number };

export type BasemapRequest =
  /** The page's request headers (`setRequestHeaders`) */
  | { type: 'headers'; rules: HeaderRules }
  | {
      type: 'init';
      /** Renderers by key: each draws its own layers of the same source */
      key: string;
      tiles: string;
      maxzoom: number;
      layers: StyleLayer[];
      /** The style's raster sources, by name, for its raster layers */
      rasters?: Record<string, RasterSource>;
      /** The style's sprites, for the fill patterns (loaded once by the page) */
      sprites?: Sprite[];
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
  sprites: Sprite[];
  /** Fill patterns by icon name, cut from the sprites once */
  patterns: Map<string, OffscreenCanvas | undefined>;
}

/** A sprite of the style: its images by name (`id:name` but in the default sprite) and its sheet */
export interface Sprite {
  image: ImageBitmap;
  icons: Record<string, { x: number; y: number; width: number; height: number; pixelRatio?: number }>;
}

/** An image of the sprites and its sheet */
const spriteIcon = (r: Renderer, name: string) => {
  const sprite = r.sprites.find((sp) => Boolean(sp.icons[name]));
  const icon = sprite?.icons[name];

  return sprite && icon ? { icon, image: sprite.image } : undefined;
};

const renderers = new Map<string, Renderer>();
/** Decoded tiles kept by the pool, shared among its workers (a vector tile always goes to the same worker): some
 * 2 MB each */
const CACHE = 96;
/** Decoded tiles by URL; this worker's share of them, set by the renderers */
const cache = new Lru<string, Promise<Map<string, TileLayer> | undefined>>(CACHE);

/** The decoded tiles one worker of `workers` keeps */
export const cacheShare = (workers: number): number => {
  return Math.max(16, Math.ceil(CACHE / Math.max(1, workers)));
};
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
const image = (url: string, priority: Priority = 'auto'): Promise<ImageBitmap | undefined> => {
  const hit = images.get(url);

  if (hit) return hit;

  const pending = (async () => {
    // A tile of a PMTiles archive (`pmtiles://…/{z}/{x}/{y}`)
    const archived = pmtilesTile(url);

    if (archived) {
      const bytes = await archived;

      return bytes && createImageBitmap(new Blob([bytes as Uint8Array<ArrayBuffer>]));
    }

    const init: PriorityInit = { priority };
    const response = await fetch(url, withHeaders(url, init));

    if (response.status === 404 || response.status === 204) return undefined;
    if (!response.ok) throw new Error(`${url}: ${response.status}`);

    return createImageBitmap(await response.blob());
  })();

  pending.catch(() => images.delete(url));
  images.set(url, pending);

  return pending;
};

/** `RequestInit` with the fetch priority, which the TypeScript DOM library may not know yet */
type PriorityInit = RequestInit & { priority?: Priority };

/** A vector tile, decoded (shared by every renderer of its URL); the first request's priority fetches it */
const tile = (url: string, priority: Priority = 'auto'): Promise<Map<string, TileLayer> | undefined> => {
  const hit = cache.get(url);

  if (hit) return hit;

  const pending = (async () => {
    // A tile of a PMTiles archive, ungzipped by the reader
    const archived = pmtilesTile(url);

    if (archived) {
      const bytes = await archived;

      return bytes && decode(bytes);
    }
    for (let attempt = 0; ; attempt += 1) {
      const init: PriorityInit = { priority };
      const response = await fetch(url, withHeaders(url, init));

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
};

/** The vector tile to draw a raster tile from, and where the raster tile lies in it (tile units) */
const source = (r: Renderer, z: number, x: number, y: number) => {
  const vz = Math.max(0, Math.min(z - 1, r.maxzoom));
  const d = z - vz;
  const n = 2 ** Math.max(0, d);
  const [vx, vy] = d >= 0 ? [Math.floor(x / n), Math.floor(y / n)] : [x, y];
  const url = r.tiles.replace('{z}', String(vz)).replace('{x}', String(vx)).replace('{y}', String(vy));
  // Fraction of the vector tile covered, and the offset in it
  const part = 1 / n;

  return { url, vz, vx, vy, part, ox: (x % n) * part, oy: (y % n) * part };
};

const value = (layer: StyleLayer, group: 'paint' | 'layout', key: string, ctx: Context, fallback: unknown) => {
  const v = layer[group]?.[key];

  if (v === undefined) return fallback;

  // An array that is not an expression is a literal value (`text-font`, `text-offset`, `line-dasharray`)
  const out = Array.isArray(v) && !isExpression(v) ? v : evaluate(v, ctx);

  return out === undefined || out === null ? fallback : out;
};

/** A color with an opacity, for the canvas */
const paint = (c: unknown, opacity: number): string => {
  const rgba = color(c) ?? [0, 0, 0, 1];

  return css([rgba[0], rgba[1], rgba[2], rgba[3] * opacity]);
};

const shown = (layer: StyleLayer, zoom: number): boolean => {
  if (layer.layout?.visibility === 'none') return false;

  return zoom >= (layer.minzoom ?? 0) && zoom < (layer.maxzoom ?? 24);
};

/** The image of a fill pattern, cut from the sprite on first use */
const patternImage = (r: Renderer, name: string): OffscreenCanvas | undefined => {
  if (r.patterns.has(name)) return r.patterns.get(name);

  const cut = (): OffscreenCanvas | undefined => {
    const found = spriteIcon(r, name);

    if (!found || !(found.icon.width > 0) || !(found.icon.height > 0)) return undefined;

    const { icon } = found;
    const canvas = new OffscreenCanvas(icon.width, icon.height);

    canvas
      .getContext('2d')
      ?.drawImage(found.image, icon.x, icon.y, icon.width, icon.height, 0, 0, icon.width, icon.height);

    return canvas;
  };
  const image = cut();

  r.patterns.set(name, image);

  return image;
};

/** Canvases free for the next tiles, by size: one per tile being drawn at most */
const canvases = new Map<number, OffscreenCanvas[]>();

/** A blank canvas of `size` pixels and its context, from the free ones if any */
const takeCanvas = (size: number): { canvas: OffscreenCanvas; g: OffscreenCanvasRenderingContext2D } => {
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
};

const giveCanvas = (canvas: OffscreenCanvas): void => {
  const free = canvases.get(canvas.width) ?? [];

  canvases.set(canvas.width, free);
  if (free.length < 4) free.push(canvas);
};

/**
 * The image of a drawn canvas, its alpha straight (the engine blends straight alpha). An opaque tile (a background
 * of full opacity, as a basemap's) is moved out of the canvas as it is, premultiplied and straight being the same
 * there; another one is read back unpremultiplied, copied
 */
export const tileImage = (
  canvas: OffscreenCanvas,
  g: OffscreenCanvasRenderingContext2D,
  opaque: boolean,
): Promise<ImageBitmap> => {
  if (opaque) return Promise.resolve(canvas.transferToImageBitmap());

  return createImageBitmap(g.getImageData(0, 0, canvas.width, canvas.height), { premultiplyAlpha: 'none' });
};

const render = async (r: Renderer, z: number, x: number, y: number, size: number, id: number, priority?: Priority) => {
  const s = source(r, z, x, y);
  const layers = await tile(s.url, priority);

  if (aborted.has(id)) return undefined;

  const { canvas, g } = takeCanvas(size);

  try {
    return await draw(r, s, layers, canvas, g, z, x, y, size, id, priority);
  } finally {
    giveCanvas(canvas);
  }
};

const draw = async (
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
) => {
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
      const known = dependent.get(key);
      const d = known ?? isFeatureDependent(layer[group]?.[key]);

      if (known === undefined) dependent.set(key, d);

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

        const icon = typeof pattern === 'string' ? spriteIcon(r, pattern)?.icon : undefined;
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
};

/** How a layer's text or icon (`prefix`) meets the others: allowed to overlap, ignored by the placement */
const overlapOf = (layer: StyleLayer, prefix: 'text' | 'icon', ctx: Context): CandidateOverlap => {
  const allow = Boolean(value(layer, 'layout', `${prefix}-allow-overlap`, ctx, false));
  const ignore = Boolean(value(layer, 'layout', `${prefix}-ignore-placement`, ctx, false));

  if (allow && ignore) return true;
  if (allow) return 'allow';

  return ignore ? 'ignore' : false;
};

/** A pair of numbers of a style value, `fallback` if it is not one */
const pair = (v: unknown, fallback: [number, number]): [number, number] => {
  return Array.isArray(v) ? [Number(v[0] ?? fallback[0]), Number(v[1] ?? fallback[1])] : fallback;
};

/** Top, right, bottom, left of one to four numbers, as CSS's sides */
const sides = (v: unknown): [number, number, number, number] => {
  const n = (Array.isArray(v) ? v : [v]).map((x) => Number(x) || 0);
  const [top = 0, right = top, bottom = top, left = right] = n;

  return [top, right, bottom, left];
};

/** A label's text in the case of its `text-transform` */
const cased = (t: string, transform: unknown): string => {
  if (transform === 'uppercase') return t.toUpperCase();
  if (transform === 'lowercase') return t.toLowerCase();

  return t;
};

/** Points every `spacing` along a line (tile units, x and y interleaved), the first half a spacing in (the middle of a
 * shorter line): each point and the segment it lies on (x, y, x0, y0, x1, y1) */
export const lineAnchors = (line: readonly number[], spacing: number): number[][] => {
  let length = 0;

  for (let i = 2; i + 1 < line.length; i += 2) {
    length += Math.hypot((line[i] ?? 0) - (line[i - 2] ?? 0), (line[i + 1] ?? 0) - (line[i - 1] ?? 0));
  }

  const out: number[][] = [];

  if (!(length > 0) || !(spacing > 0)) return out;

  let next = Math.min(spacing / 2, length / 2);
  let walked = 0;

  for (let i = 2; i + 1 < line.length; i += 2) {
    const [ax, ay, bx, by] = [line[i - 2] ?? 0, line[i - 1] ?? 0, line[i] ?? 0, line[i + 1] ?? 0];
    const d = Math.hypot(bx - ax, by - ay);

    while (d > 0 && next <= walked + d) {
      const t = (next - walked) / d;

      out.push([ax + (bx - ax) * t, ay + (by - ay) * t, ax, ay, bx, by]);
      next += spacing;
    }
    walked += d;
  }

  return out;
};

/** Longitude and latitude of a point of tile (z, x, y) in tile units */
const lonLat = (z: number, x: number, y: number, extent: number, px: number, py: number): [number, number] => {
  const n = 2 ** z;

  return [longitude((x + px / extent) / n), latitude((y + py / extent) / n)];
};

/** The icon of a label: its layer's icon properties for the feature */
const iconOf = (
  layer: StyleLayer,
  image: string,
  ctx: Context,
  placement: 'point' | 'line',
  sizeOf: (layer: StyleLayer) => Json,
): CandidateIcon => {
  const alignment = value(layer, 'layout', 'icon-rotation-alignment', ctx, 'auto');
  const fit = String(value(layer, 'layout', 'icon-text-fit', ctx, 'none'));
  const padding = value(layer, 'layout', 'icon-text-fit-padding', ctx, [0, 0, 0, 0]);

  return {
    image,
    size: isFeatureDependent(layer.layout?.['icon-size'])
      ? Number(value(layer, 'layout', 'icon-size', ctx, 1))
      : sizeOf(layer),
    anchor: String(value(layer, 'layout', 'icon-anchor', ctx, 'center')),
    offset: pair(value(layer, 'layout', 'icon-offset', ctx, [0, 0]), [0, 0]),
    rotate: Number(value(layer, 'layout', 'icon-rotate', ctx, 0)) || 0,
    alignment: alignment === 'map' || (alignment === 'auto' && placement === 'line') ? 'map' : 'viewport',
    overlap: overlapOf(layer, 'icon', ctx),
    optional: Boolean(value(layer, 'layout', 'icon-optional', ctx, false)),
    fit: fit === 'width' || fit === 'height' || fit === 'both' ? fit : 'none',
    padding: sides(padding),
    color: String(value(layer, 'paint', 'icon-color', ctx, '#000')),
    opacity: Number(value(layer, 'paint', 'icon-opacity', ctx, 1)),
  };
};

const labels = async (
  r: Renderer,
  z: number,
  x: number,
  y: number,
  id: number,
  priority?: Priority,
  zoom = z,
): Promise<LabelCandidate[]> => {
  const url = r.tiles.replace('{z}', String(z)).replace('{x}', String(x)).replace('{y}', String(y));
  const layers = await tile(url, priority);

  if (!layers || aborted.has(id)) return [];

  const out: LabelCandidate[] = [];
  // The text size of a layer when it depends on the zoom only: legacy stops as they are, an expression as stops
  const sizes = new Map<StyleLayer, Json>();
  const sizeOf = (layer: StyleLayer): Json => {
    const known = sizes.get(layer);

    if (known !== undefined) return known;

    const v = layer.layout?.['text-size'] ?? 16;
    const s = isExpression(v) ? zoomStops(v) : v;

    sizes.set(layer, s);

    return s;
  };
  // The icon size of a layer when it depends on the zoom only, as the text size
  const iconSizes = new Map<StyleLayer, Json>();
  const iconSizeOf = (layer: StyleLayer): Json => {
    const known = iconSizes.get(layer);

    if (known !== undefined) return known;

    const v = layer.layout?.['icon-size'] ?? 1;
    const s = isExpression(v) ? zoomStops(v) : v;

    iconSizes.set(layer, s);

    return s;
  };

  r.layers.forEach((layer, index) => {
    const layout = layer.layout ?? {};

    if (layer.type !== 'symbol' || !shown(layer, zoom)) return;
    if (layout['text-field'] === undefined && layout['icon-image'] === undefined) return;

    const data = layers.get(layer['source-layer'] ?? '');

    if (!data) return;

    const zctx: Context = { zoom, properties: {}, type: 'Unknown' };
    const placement = value(layer, 'layout', 'symbol-placement', zctx, 'point') === 'line' ? 'line' : 'point';
    const textAlong =
      placement === 'line' && value(layer, 'layout', 'text-rotation-alignment', zctx, 'auto') !== 'viewport';

    for (const f of data.features) {
      const ctx = context(f, zoom);

      if (!filter(layer.filter, ctx)) continue;

      const field = text(layout['text-field'], ctx).trim();
      const image = text(layout['icon-image'], ctx).trim();

      if (!field && !image) continue;

      const t = cased(field, value(layer, 'layout', 'text-transform', ctx, 'none'));
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
        overlap: overlapOf(layer, 'text', ctx),
        ...(textAlong ? { textAlong } : {}),
        ...(value(layer, 'layout', 'text-optional', ctx, false) ? { textOptional: true } : {}),
        ...(image ? { icon: iconOf(layer, image, ctx, placement, iconSizeOf) } : {}),
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
      } else if (!t && placement === 'line') {
        // Icons alone along lines (one-way arrows): every `symbol-spacing` CSS pixels (a tile is 512 across at its
        // level, twice as many at each zoom past it)
        const spacing =
          (Number(value(layer, 'layout', 'symbol-spacing', ctx, 250)) * data.extent) / (512 * 2 ** (zoom - z));

        for (const line of f.geometry) {
          for (const [px = 0, py = 0, ax = 0, ay = 0, bx = 0, by = 0] of lineAnchors(line, Math.max(spacing, 1))) {
            if (px < 0 || py < 0 || px >= data.extent || py >= data.extent) continue;

            const [lon, lat] = lonLat(z, x, y, data.extent, px, py);
            const [lon1, lat1] = lonLat(z, x, y, data.extent, bx, by);
            const [lon0, lat0] = lonLat(z, x, y, data.extent, ax, ay);

            out.push({ ...base, lon, lat, along: [lon1 - lon0, lat1 - lat0] });
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
};

const post = (message: BasemapResponse, transfer: Transferable[] = []) =>
  (self as unknown as Worker).postMessage(message, transfer);

self.onmessage = async ({ data }: MessageEvent<BasemapRequest>) => {
  if (data.type === 'headers') {
    setHeaderRules(data.rules);

    return;
  }
  if (data.type === 'init') {
    if (data.workers !== undefined) cache.capacity = cacheShare(data.workers);
    renderers.set(data.key, {
      tiles: data.tiles,
      maxzoom: data.maxzoom,
      layers: data.layers,
      rasters: data.rasters ?? {},
      sprites: data.sprites ?? [],
      patterns: new Map(),
    });

    return;
  }
  if (data.type === 'drop') {
    // Its copy of the sprites
    renderers.get(data.key)?.sprites.forEach((sprite) => {
      sprite.image.close();
    });
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
      post({ id: data.id, labels: await labels(r, data.z, data.x, data.y, data.id, data.priority, data.zoom) });
    }
  } catch (error) {
    post({ id: data.id, error: String(error) });
  } finally {
    active.delete(data.id);
    aborted.delete(data.id);
  }
};

/** A tile of another layer: the altitudes of a relief tile, or a reprojected raster tile */
const decodeTile = async (data: Extract<BasemapWork, { type: 'heights' | 'reproject' }> & { id: number }) => {
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
};
