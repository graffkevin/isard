/**
 * Vector basemaps from a MapLibre style (OpenFreeMap's Positron by default): the fills, lines, circles and raster
 * layers drawn tile by tile in workers into the raster pool, so that they lie on the relief like any basemap and composite
 * with the hillshade in one pass; and the labels of the style's symbol layers in a symbol layer of their own, kept
 * above the application's layers (as MapLibre's `beforeId: firstSymbol` does), shaped and placed by the engine.
 *
 * Any subset of the style can be drawn on its own raster layer (`raster({ layers })`), at its own place in the stack:
 * rivers from the basemap's data over a flood layer, for instance, restyled with `extraLayers`.
 */
import type { BasemapRequest, BasemapResponse, BasemapWork, Priority, Sprite } from './basemap.worker.ts';
import { OPENFREEMAP_ATTRIBUTION } from './basemaps.ts';
import type { Feature, Size } from './features.ts';
import { IGN_ATTRIBUTION } from './geoplateforme.ts';
import type { Layer, RasterLayer } from './layers.ts';
import type { GeoMap } from './map.ts';
import type { Anchor } from './overlay.ts';
import type { SymbolLayer } from './symbols.ts';
import {
  type Json,
  type LabelCandidate,
  labelFont,
  type RasterSource,
  type Style,
  type StyleLayer,
} from './vectorstyle.ts';
import type { Camera } from './wasm/isard.js';

/** OpenFreeMap's styles: OpenStreetMap data in the OpenMapTiles schema, worldwide, free and without a key */
export const OPENFREEMAP_STYLES = {
  positron: 'https://tiles.openfreemap.org/styles/positron',
  liberty: 'https://tiles.openfreemap.org/styles/liberty',
  bright: 'https://tiles.openfreemap.org/styles/bright',
} as const;

/** PLAN.IGN vector styles of the Géoplateforme (France) */
export const PLAN_STYLES = {
  standard: 'https://data.geopf.fr/annexes/ressources/vectorTiles/styles/PLAN.IGN/standard.json',
  grey: 'https://data.geopf.fr/annexes/ressources/vectorTiles/styles/PLAN.IGN/gris.json',
} as const;

/** Vector basemaps by name (see `BasemapName`) */
export const VECTOR_BASEMAPS = {
  'osm-positron': OPENFREEMAP_STYLES.positron,
  'osm-liberty': OPENFREEMAP_STYLES.liberty,
  'osm-bright': OPENFREEMAP_STYLES.bright,
  'ign-plan-vector': PLAN_STYLES.standard,
  'ign-plan-grey-vector': PLAN_STYLES.grey,
} as const;

export interface VectorTilesOptions {
  /** A style URL or object (default OpenFreeMap's Positron) */
  style?: string | Style;
  /** Workers drawing the tiles (default cores - 1, 1 to 4). The vector tiles of a map share its workers (and their
   * decoded tiles); another count gives these tiles workers of their own */
  workers?: number;
  /** Where to load the worker from, when a bundler does not resolve the default URL (default the map's
   * `basemapWorkerUrl`); another URL than the map's gives these tiles workers of their own */
  workerUrl?: string | URL;
  /** Credits of the tiles, shown while its raster layers are on the map (default: OpenFreeMap's for its styles, the
   * IGN's for PLAN.IGN's) */
  attribution?: string;
}

/** The credits of a known style: OpenFreeMap's, PLAN.IGN's */
export function styleAttribution(style: string | Style): string | undefined {
  if (typeof style !== 'string') return undefined;
  if ((Object.values(OPENFREEMAP_STYLES) as string[]).includes(style)) return OPENFREEMAP_ATTRIBUTION;
  if ((Object.values(PLAN_STYLES) as string[]).includes(style)) return IGN_ATTRIBUTION;
  return undefined;
}

/** Text size of a style (a number, legacy stops, or the stops of a zoom expression made by the worker) as a size for
 * the symbol layer */
function size(value: Json): Size {
  if (typeof value === 'number') return value;
  if (Array.isArray(value) && value.length > 0 && value.every(Array.isArray)) {
    return value.map((s) => [Number((s as Json[])[0]), Number((s as Json[])[1])] as [number, number]);
  }
  if (value && typeof value === 'object' && !Array.isArray(value) && Array.isArray(value.stops)) {
    return (value.stops as Json[]).map((s) => {
      const [z, v] = s as [number, number];
      return [Number(z), Number(v)] as [number, number];
    });
  }
  return 16;
}

/** Fetches a JSON document (a style, a TileJSON) */
async function json<T>(url: string): Promise<T> {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`${url}: ${response.status}`);
  return (await response.json()) as T;
}

/** Default size of a basemap worker pool: cores - 1, 1 to 4 */
export function defaultWorkerCount(): number {
  return Math.min(4, Math.max(1, (typeof navigator !== 'undefined' ? navigator.hardwareConcurrency || 2 : 2) - 1));
}

/** Default URL of the basemap worker, next to the module */
export function defaultWorkerUrl(): URL {
  return new URL('./basemap.worker.js', import.meta.url);
}

/** Fetch priority of a raster tile drawn by the workers: 'high' near the center of the view, at the current zoom or
 * coarser; 'low' far from it or finer (prefetch); 'auto' between */
export function tilePriority(z: number, x: number, y: number, camera: Pick<Camera, 'center' | 'zoom'>): Priority {
  // A raster tile of level z covers the map at zoom z - 1
  if (z - 1 > camera.zoom + 1) return 'low';
  const n = 2 ** z;
  const [lon, lat] = camera.center;
  const cx = ((lon + 180) / 360) * n;
  const phi = (Math.max(-85.05, Math.min(85.05, lat)) * Math.PI) / 180;
  const cy = ((1 - Math.log(Math.tan(phi) + 1 / Math.cos(phi)) / Math.PI) / 2) * n;
  // Across the antimeridian, the short way
  let dx = Math.abs(x + 0.5 - cx) % n;
  dx = Math.min(dx, n - dx);
  const d = Math.hypot(dx, y + 0.5 - cy);
  if (d <= 1.5) return 'high';
  return d >= 4 ? 'low' : 'auto';
}

/** The worker of a vector tile among `count`: always the same one */
export function workerIndex(vz: number, vx: number, vy: number, count: number): number {
  // Unsigned: every worker gets its share
  return ((Math.imul(vx, 73856093) ^ Math.imul(vy, 19349663) ^ vz) >>> 0) % Math.max(1, count);
}

/** The worker-side key of a renderer: unique across the `VectorTiles` sharing a pool */
export function rendererKey(namespace: string, n: number): string {
  return `${namespace}:r${n}`;
}

/** What the pool needs of a worker (a `Worker`, or a stand-in in tests) */
export type PoolWorker = Pick<Worker, 'postMessage' | 'terminate' | 'onmessage' | 'onerror' | 'onmessageerror'>;

/**
 * @internal Workers drawing vector tiles, shared by the `VectorTiles` of a map: each vector tile always goes to the
 * same worker, which keeps it decoded for every style drawn from it. Requests are tracked one by one, with their
 * owner, so that one `VectorTiles` going away cancels its own requests only.
 */
export class BasemapWorkers {
  private readonly workers: PoolWorker[];
  private readonly pending = new Map<
    number,
    { resolve: (r: BasemapResponse) => void; reject: (e: Error) => void; worker: PoolWorker; owner: object }
  >();
  private next = 1;
  private namespaces = 0;
  private terminated = false;

  constructor(
    readonly count: number,
    readonly url: string | URL,
    create: (url: string | URL) => PoolWorker = (u) => new Worker(u, { type: 'module' }),
  ) {
    this.workers = Array.from({ length: Math.max(1, count) }, () => {
      const worker = create(url);
      worker.onmessage = ({ data }: MessageEvent<BasemapResponse>) => {
        const p = this.pending.get(data.id);
        if (!p) return;
        this.pending.delete(data.id);
        if ('error' in data) p.reject(new Error(data.error));
        else p.resolve(data);
      };
      // A worker that fails to load (a bundler serving it from elsewhere) or to read a message fails its requests
      worker.onerror = (event) => {
        event.preventDefault();
        this.fail(worker, `the vector basemap worker ${url} failed${event.message ? `: ${event.message}` : ''}`);
      };
      worker.onmessageerror = () => this.fail(worker, `the vector basemap worker ${url} sent an unreadable message`);
      return worker;
    });
  }

  /** A prefix for the renderer keys of one `VectorTiles` */
  namespace(): string {
    return `v${this.namespaces++}`;
  }

  /** Requests in progress */
  get size(): number {
    return this.pending.size;
  }

  /** Rejects the requests of a failed worker */
  private fail(worker: PoolWorker, message: string): void {
    const error = new Error(
      `isard: ${message}. If a bundler moves the worker, give its URL in the \`basemapWorkerUrl\` option (see "With a bundler" in the guide)`,
    );
    for (const [id, p] of this.pending) {
      if (p.worker !== worker) continue;
      this.pending.delete(id);
      p.reject(error);
    }
  }

  /** The worker of a vector tile */
  worker(vz: number, vx: number, vy: number): PoolWorker {
    return this.workers[workerIndex(vz, vx, vy, this.workers.length)] ?? (this.workers[0] as PoolWorker);
  }

  /** Sends a message to every worker (a renderer, or its removal) */
  broadcast(message: Extract<BasemapRequest, { type: 'init' } | { type: 'drop' }>): void {
    if (this.terminated) return;
    // A renderer tells the workers how many they are: they share the decoded tiles' budget
    const sent = message.type === 'init' ? { ...message, workers: this.workers.length } : message;
    // A sprite image is copied to each worker (structured clone)
    for (const w of this.workers) w.postMessage(sent);
  }

  /** Asks a worker for a tile's image or labels, on behalf of `owner` */
  request(worker: PoolWorker, work: BasemapWork, owner: object, signal?: AbortSignal): Promise<BasemapResponse> {
    return new Promise<BasemapResponse>((resolve, reject) => {
      if (this.terminated) return reject(new DOMException('isard: vector tiles destroyed', 'AbortError'));
      if (signal?.aborted) return reject(new DOMException('aborted', 'AbortError'));
      const id = this.next++;
      const abort = () => {
        if (!this.pending.delete(id)) return;
        worker.postMessage({ type: 'abort', id } satisfies BasemapRequest);
        reject(new DOMException('aborted', 'AbortError'));
      };
      // The listener goes once the request is settled
      const settle = () => signal?.removeEventListener('abort', abort);
      this.pending.set(id, {
        resolve: (r) => {
          settle();
          resolve(r);
        },
        reject: (e) => {
          settle();
          reject(e);
        },
        worker,
        owner,
      });
      signal?.addEventListener('abort', abort, { once: true });
      worker.postMessage({ ...work, id } satisfies BasemapRequest);
    });
  }

  /** Cancels the requests of an owner (the workers stop them) */
  cancel(owner: object): void {
    for (const [id, p] of this.pending) {
      if (p.owner !== owner) continue;
      this.pending.delete(id);
      p.worker.postMessage({ type: 'abort', id } satisfies BasemapRequest);
      p.reject(new DOMException('isard: vector tiles destroyed', 'AbortError'));
    }
  }

  /** Stops the workers; their requests fail */
  terminate(): void {
    if (this.terminated) return;
    this.terminated = true;
    for (const w of this.workers) w.terminate();
    const pending = [...this.pending.values()];
    this.pending.clear();
    for (const p of pending) p.reject(new DOMException('isard: vector tiles destroyed', 'AbortError'));
  }
}

export class VectorTiles {
  private readonly pool: BasemapWorkers;
  /** The pool is this instance's own (a worker count or URL other than the map's): stopped with it */
  private readonly ownPool: boolean;
  /** Prefix of its renderer keys in the pool's workers */
  private readonly namespace: string;
  private destroyed = false;
  private keys = 0;
  /** Renderers sent to the workers, dropped on destroy */
  private readonly renderers: string[] = [];
  private readonly cleanup: (() => void)[] = [];
  /** Credits of its raster layers */
  private readonly attribution: string | undefined;
  /** Its raster and label layers, removed with it */
  private readonly layers: Layer[] = [];
  /** The style once loaded, with its source resolved */
  readonly ready: Promise<{ style: Style; tiles: string; maxzoom: number; rasters: Record<string, RasterSource> }>;

  /**
   * Its workers are the map's (shared by every `VectorTiles` of the map, a vector tile decoded once for all of them),
   * unless `workers` or `workerUrl` asks for others than the map's: then it has workers of its own
   */
  constructor(
    private readonly map: GeoMap,
    options: VectorTilesOptions = {},
  ) {
    const { pool, own } = map.basemapWorkers(options.workers, options.workerUrl);
    this.pool = pool;
    this.ownPool = own;
    this.namespace = pool.namespace();
    const style = options.style ?? OPENFREEMAP_STYLES.positron;
    this.attribution = options.attribution ?? styleAttribution(style);
    this.ready = this.load(style);
    // Reported once here; `ready`, the raster tiles and the labels fail with it
    this.ready.catch((error: unknown) => {
      if (!this.destroyed) console.warn('isard: vector tiles', error);
    });
  }

  private async load(input: string | Style) {
    const style = typeof input === 'string' ? await json<Style>(input) : input;
    const [name, source] = Object.entries(style.sources).find(([, s]) => s.type === 'vector') ?? [];
    if (!name || !source) throw new Error('the style has no vector source');
    let { tiles, maxzoom } = source;
    if (!tiles && source.url) {
      const tilejson = await json<{ tiles: string[]; maxzoom?: number }>(source.url);
      tiles = tilejson.tiles;
      maxzoom ??= tilejson.maxzoom;
    }
    const template = tiles?.[0];
    if (!template) throw new Error(`source ${name} has no tiles`);
    // Its raster sources of tiles, drawn into the same images (the shaded relief of a style at low zoom)
    const rasters: Record<string, RasterSource> = {};
    for (const [key, s] of Object.entries(style.sources)) {
      const url = s.type === 'raster' ? s.tiles?.[0] : undefined;
      if (url) rasters[key] = { tiles: url, maxzoom: s.maxzoom ?? 22, tileSize: s.tileSize ?? 512 };
    }
    // Only the layers of those sources (and the background); other types (hillshade, heatmap…) are left out. The
    // application's style object stays as it is
    const layers = style.layers.filter(
      (l) =>
        l.type === 'background' ||
        (l.source === name && l.type !== 'raster') ||
        (l.type === 'raster' && l.source !== undefined && rasters[l.source] !== undefined),
    );
    return { style: { ...style, layers }, tiles: template, maxzoom: maxzoom ?? 14, rasters };
  }

  /** The fonts of the style's labels, for its glyphs (the first of each `text-font`, as the workers take it) */
  private static fonts(style: Style): string[] {
    const fonts = new Set<string>();
    for (const layer of style.layers) {
      const font = layer.layout?.['text-font'];
      if (layer.type === 'symbol' && Array.isArray(font) && typeof font[0] === 'string') fonts.add(labelFont(font[0]));
    }
    return [...fonts];
  }

  /** The style's sprite (its JSON and its image), loaded once, for the renderers that draw fill patterns */
  private sprite: Promise<Sprite | undefined> | undefined;

  private loadSprite(url: string): Promise<Sprite | undefined> {
    this.sprite ??= (async () => {
      try {
        const [json, png] = await Promise.all([fetch(`${url}.json`), fetch(`${url}.png`)]);
        if (!json.ok || !png.ok) return undefined;
        return { icons: await json.json(), image: await createImageBitmap(await png.blob()) };
      } catch {
        return undefined;
      }
    })();
    return this.sprite;
  }

  private request(vz: number, vx: number, vy: number, work: BasemapWork, signal?: AbortSignal) {
    if (this.destroyed) return Promise.reject(new DOMException('isard: vector tiles destroyed', 'AbortError'));
    return this.pool.request(this.pool.worker(vz, vx, vy), work, this, signal);
  }

  /** Sends a renderer of these layers to every worker; returns its key */
  private async renderer(select: (layer: StyleLayer) => boolean, extra: StyleLayer[]) {
    const { style, tiles, maxzoom, rasters } = await this.ready;
    const key = rendererKey(this.namespace, this.keys++);
    const layers = [...style.layers.filter(select), ...extra];
    const patterns = layers.some((l) => l.paint?.['fill-pattern'] !== undefined);
    // The image is downloaded once, copied to each worker
    const sprite = patterns && style.sprite ? await this.loadSprite(style.sprite) : undefined;
    if (this.destroyed) throw new DOMException('isard: vector tiles destroyed', 'AbortError');
    this.pool.broadcast({ type: 'init', key, tiles, maxzoom, layers, rasters, ...(sprite ? { sprite } : {}) });
    this.renderers.push(key);
    return { key, maxzoom };
  }

  /**
   * A raster layer drawing the style's fills, lines and circles (or those `layers` keeps, then `extraLayers`, of the
   * same source); `size`: texture size of its tiles (512 by default: sharp lines)
   */
  raster(
    options: {
      layers?: (layer: StyleLayer) => boolean;
      extraLayers?: StyleLayer[];
      size?: number;
      opacity?: number;
      minZoom?: number;
      maxZoom?: number;
      before?: Parameters<GeoMap['addRaster']>[0]['before'];
    } = {},
  ): RasterLayer {
    const { layers = () => true, extraLayers = [], size: texture = 512, opacity = 1, before } = options;
    const drawn = (l: StyleLayer) => l.type !== 'symbol' && layers(l);
    const renderer = this.renderer(drawn, extraLayers);
    // Its failure is reported by `ready` and each tile
    renderer.catch(() => undefined);
    const loader = async (z: number, x: number, y: number, signal: AbortSignal) => {
      const { key, maxzoom } = await renderer;
      // The vector tile it is drawn from (see the worker)
      const vz = Math.max(0, Math.min(z - 1, maxzoom));
      const shift = z - vz;
      const priority = tilePriority(z, x, y, this.map.getCamera());
      const response = await this.request(
        vz,
        x >> shift,
        y >> shift,
        { type: 'render', key, z, x, y, size: texture, priority },
        signal,
      );
      return 'image' in response ? response.image : null;
    };
    const layer = this.map.addRaster({
      url: '',
      size: texture,
      opacity,
      minZoom: options.minZoom ?? 0,
      maxZoom: options.maxZoom ?? 20,
      loader,
      ...(this.attribution ? { attribution: this.attribution } : {}),
      ...(before ? { before } : {}),
    });
    this.layers.push(layer);
    return layer;
  }

  /**
   * A symbol layer with the labels of the style's symbol layers (or those `layers` keeps), updated as the view
   * moves: the labels of the vector tiles in view, the higher style layers placed first. It stays above the layers
   * added after it unless one is moved over it.
   */
  labels(options: { layers?: (layer: StyleLayer) => boolean; maxTiles?: number } = {}): SymbolLayer<LabelCandidate> {
    const { layers = () => true, maxTiles = 36 } = options;
    const symbol = this.map.addSymbols<LabelCandidate>({
      data: [],
      text: (p) => p.text,
      font: (p) => p.font,
      size: (p) => size(p.size),
      color: (p) => p.color,
      haloColor: (p) => p.haloColor,
      haloWidth: (p) => p.haloWidth,
      anchor: (p) => p.anchor as Anchor,
      offset: (p) => p.offset,
      maxWidth: (p) => p.maxWidth,
      lineHeight: (p) => p.lineHeight,
      overlap: (p) => p.overlap,
      placement: 'line',
    });
    this.map.keepOnTop(symbol);
    this.layers.push(symbol);
    const renderer = this.ready.then(({ style }) => {
      // The style's fonts from its own glyphs
      if (style.glyphs) this.map.glyphs.register(style.glyphs, VectorTiles.fonts(style));
      return this.renderer((l) => l.type === 'symbol' && layers(l), []);
    });
    renderer.catch(() => undefined);
    /** The labels of the tiles, the most recently used last */
    const tiles = new Map<string, Promise<LabelCandidate[]>>();
    let generation = 0;
    /** The tiles whose labels are shown */
    let shown = '';
    const update = async () => {
      if (this.destroyed) return;
      const { key, maxzoom } = await renderer;
      const run = ++generation;
      const camera = this.map.getCamera();
      const z = Math.max(0, Math.min(Math.floor(camera.zoom), maxzoom));
      const n = 2 ** z;
      const [west, south, east, north] = this.map.getBounds();
      const tx = (lon: number) => Math.floor(((lon + 180) / 360) * n);
      const ty = (lat: number) => {
        const phi = (Math.max(-85.05, Math.min(85.05, lat)) * Math.PI) / 180;
        return Math.floor(((1 - Math.log(Math.tan(phi) + 1 / Math.cos(phi)) / Math.PI) / 2) * n);
      };
      const [cx, cy] = [((camera.center[0] + 180) / 360) * n, ty(camera.center[1])];
      // The tiles of the bounds, within a window around the center: a tilted view's bounds reach far, and only the
      // nearest `maxTiles` are kept anyway
      const reach = Math.ceil(Math.sqrt(maxTiles)) + 2;
      const [x0, x1] = [
        Math.max(0, tx(west), Math.floor(cx) - reach),
        Math.min(n - 1, tx(east), Math.floor(cx) + reach),
      ];
      const [y0, y1] = [
        Math.max(0, ty(north), Math.floor(cy) - reach),
        Math.min(n - 1, ty(south), Math.floor(cy) + reach),
      ];
      const wanted: [number, number][] = [];
      for (let y = y0; y <= y1; y += 1) for (let x = x0; x <= x1; x += 1) wanted.push([x, y]);
      // Nearest to the center first, at most `maxTiles` (tilted views reach far)
      wanted.sort(
        (a, b) => Math.hypot(a[0] + 0.5 - cx, a[1] + 0.5 - cy) - Math.hypot(b[0] + 0.5 - cx, b[1] + 0.5 - cy),
      );
      const keys = wanted.slice(0, maxTiles).map(([x, y]) => `${z}/${x}/${y}`);
      // The same tiles as the labels shown: nothing to do
      const set = keys.join(' ');
      if (set === shown) return;
      const lists = await Promise.all(
        keys.map((k, index) => {
          let p = tiles.get(k);
          if (p) {
            // Used again: the most recent
            tiles.delete(k);
            tiles.set(k, p);
            return p;
          }
          const [tz, x, y] = k.split('/').map(Number) as [number, number, number];
          // The nearest tiles first: the others wait for the raster tiles
          const priority: Priority = index < 4 ? 'auto' : 'low';
          p = this.request(tz, x, y, { type: 'labels', key, z: tz, x, y, priority }).then(
            (r) => ('labels' in r ? r.labels : []),
            () => {
              // A network error is not kept: the tile is asked for again on the next update
              tiles.delete(k);
              return [];
            },
          );
          tiles.set(k, p);
          // A bounded cache of tiles' labels, the least recently used dropped
          if (tiles.size > 256) {
            const oldest = tiles.keys().next().value;
            if (oldest !== undefined) tiles.delete(oldest);
          }
          return p;
        }),
      );
      if (run !== generation || this.destroyed) return;
      // A tile that failed is missing: the next update asks for it again
      shown = keys.every((k) => tiles.has(k)) ? set : '';
      const features: Feature<LabelCandidate>[] = lists
        .flat()
        // The labels of the higher style layers first: they win the collisions
        .sort((a, b) => b.layer - a.layer)
        .map(
          (p) =>
            ({
              type: 'Feature',
              properties: p,
              geometry: {
                type: p.along ? 'LineString' : 'Point',
                coordinates: p.along
                  ? [
                      [p.lon - p.along[0] / 2, p.lat - p.along[1] / 2],
                      [p.lon + p.along[0] / 2, p.lat + p.along[1] / 2],
                    ]
                  : [p.lon, p.lat],
              },
            }) as Feature<LabelCandidate>,
        );
      symbol.setData(features);
    };
    // The style failed to load: reported by `ready`
    const refresh = () => {
      update().catch(() => undefined);
    };
    let timer = 0;
    const schedule = () => {
      if (timer) return;
      timer = window.setTimeout(() => {
        timer = 0;
        refresh();
      }, 250);
    };
    this.cleanup.push(this.map.on('move', schedule), this.map.on('moveend', refresh), () => clearTimeout(timer));
    refresh();
    return symbol;
  }

  /** Stops its workers' work and removes its raster and label layers from the map (those still there) */
  destroy(): void {
    if (this.destroyed) return;
    this.destroyed = true;
    for (const off of this.cleanup) off();
    this.cleanup.length = 0;
    for (const layer of this.layers) if (this.map.hasLayer(layer)) layer.remove();
    this.layers.length = 0;
    this.map.dropVectorTiles(this);
    // The page's copy of the sprite (the workers have theirs)
    this.sprite?.then((sprite) => sprite?.image.close()).catch(() => undefined);
    if (this.ownPool) {
      this.pool.terminate();
    } else {
      // Shared workers: only its requests stop and its renderers go
      this.pool.cancel(this);
      for (const key of this.renderers) this.pool.broadcast({ type: 'drop', key });
    }
    this.renderers.length = 0;
  }
}
