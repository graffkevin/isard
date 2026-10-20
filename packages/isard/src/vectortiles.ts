/**
 * Vector basemaps from a MapLibre style (OpenFreeMap's Positron by default): the fills, lines, circles and raster
 * layers drawn tile by tile in workers into the raster pool, so that they lie on the relief like any basemap and
 * composite with the hillshade in one pass; and the labels of the style's symbol layers in a symbol layer of their own,
 * kept above the application's layers (as MapLibre's `beforeId: firstSymbol` does), listed by the workers, shaped in
 * the page once per vector tile, placed by the engine.
 *
 * Any subset of the style can be drawn on its own raster layer (`raster({ layers })`), at its own place in the stack:
 * rivers from the basemap's data over a flood layer, for instance, restyled with `extraLayers`.
 */
import type { BasemapRequest, BasemapResponse, BasemapWork, Priority, Sprite } from './basemap.worker.ts';
import { OPENFREEMAP_ATTRIBUTION } from './basemaps.ts';
import type { Feature, Size } from './features.ts';
import { mercatorX, mercatorY } from './geo.ts';
import { IGN_ATTRIBUTION } from './geoplateforme.ts';
import { type LoadedSprite, loadSprites, readPixels } from './icons.ts';
import type { Layer, RasterLayer } from './layers.ts';
import { Lru } from './lru.ts';
import type { GeoMap } from './map.ts';
import type { Anchor } from './overlay.ts';
import { pmtiles, pmtilesUrl } from './pmtiles.ts';
import { type Job, SOURCE_CACHE } from './resample.ts';
import type { SymbolLayer } from './symbols.ts';
import type { PngEncoding } from './terrain.ts';
import type { RasterOptions } from './types.ts';
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

/** Text or icon size of a style (a number, legacy stops, or the stops of a zoom expression made by the worker) as a
 * size for the symbol layer */
export function size(value: Json, fallback = 16): Size {
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
  return fallback;
}

/** Fetches a JSON document (a style, a TileJSON) */
async function json<T>(url: string): Promise<T> {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`${url}: ${response.status}`);
  return (await response.json()) as T;
}

/**
 * A style source's tiles and deepest level: a `pmtiles://` archive (its `url`, or a `tiles` template) read for its
 * header, its tiles then named `pmtiles://<absolute archive URL>/{z}/{x}/{y}` for the workers; other sources as they are
 */
async function resolveSource(source: Style['sources'][string]): Promise<{ tiles?: string[]; maxzoom?: number }> {
  const given = source.url ?? source.tiles?.[0] ?? '';
  const archive = pmtilesUrl(given.replace(/\/\{z\}\/\{x\}\/\{y\}$/, ''));
  if (!archive)
    return {
      ...(source.tiles ? { tiles: source.tiles } : {}),
      ...(source.maxzoom === undefined ? {} : { maxzoom: source.maxzoom }),
    };
  const header = await pmtiles(archive).header();
  return { tiles: [`pmtiles://${archive}/{z}/{x}/{y}`], maxzoom: source.maxzoom ?? header.maxZoom };
}

/** Pause before the labels of tiles that failed are asked for again, when no move brings an update (ms) */
const RETRY_LABELS_MS = 3000;

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
  const cx = mercatorX(lon) * n;
  const cy = mercatorY(lat) * n;
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

  /** The altitudes of a PNG relief tile `z`/`x`/`y` at `url`, decoded by a worker (see `fetchHeights`) */
  heights(
    url: string,
    encoding: PngEncoding,
    floor: number | undefined,
    [z, x, y]: [number, number, number],
    signal: AbortSignal,
    owner: object,
  ): Promise<Float32Array | null> {
    const work: BasemapWork = { type: 'heights', url, encoding, ...(floor === undefined ? {} : { floor }) };
    return this.request(this.worker(z, x, y), work, owner, signal).then((r) => ('heights' in r ? r.heights : null));
  }

  /** Mercator tile `z`/`x`/`y` reprojected by a worker from the source images of its plan (see `resampler`) */
  reproject(
    job: Job,
    [z, x, y]: [number, number, number],
    signal: AbortSignal,
    owner: object,
  ): Promise<ImageBitmap | null> {
    // Blocks of 4 × 4 Mercator tiles go to the same worker: neighbours share source tiles, each worker keeps its share
    const worker = this.worker(Math.max(0, z - 2), x >> 2, y >> 2);
    const cached = Math.max(16, Math.ceil(SOURCE_CACHE / this.workers.length));
    const work: BasemapWork = { type: 'reproject', job, cached };
    return this.request(worker, work, owner, signal).then((r) => ('image' in r ? r.image : null));
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

/** The labels of a vector tile: features by style layer (its index in the style), the higher layers first */
type TileLabels = [layer: number, features: Feature<LabelCandidate>[]][];

/** A tile's label candidates as features, by style layer */
export function tileLabels(candidates: readonly LabelCandidate[]): TileLabels {
  const byLayer = new Map<number, Feature<LabelCandidate>[]>();
  for (const p of candidates) {
    const feature: Feature<LabelCandidate> = {
      type: 'Feature',
      properties: p,
      geometry: p.along
        ? {
            type: 'LineString',
            coordinates: [
              [p.lon - p.along[0] / 2, p.lat - p.along[1] / 2],
              [p.lon + p.along[0] / 2, p.lat + p.along[1] / 2],
            ],
          }
        : { type: 'Point', coordinates: [p.lon, p.lat] },
    };
    const list = byLayer.get(p.layer);
    if (list) list.push(feature);
    else byLayer.set(p.layer, [feature]);
  }
  return [...byLayer].sort((a, b) => b[0] - a[0]);
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
  /** Its first raster layer: the ground its labels follow */
  private ground: RasterLayer | undefined;
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
    let { tiles, maxzoom } = await resolveSource(source);
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
      if (s.type !== 'raster') continue;
      const r = await resolveSource(s);
      const url = r.tiles?.[0];
      if (url) rasters[key] = { tiles: url, maxzoom: r.maxzoom ?? 22, tileSize: s.tileSize ?? 512 };
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

  /** The style's sprites (their JSON and their images), loaded once, for the fill patterns and the icons */
  private sprites: Promise<LoadedSprite[]> | undefined;

  private loadSprites(sprite: Style['sprite']): Promise<LoadedSprite[]> {
    this.sprites ??= loadSprites(sprite).catch(() => []);
    return this.sprites;
  }

  /** Prefix of the names of its sprites' images among the map's icons */
  private get iconPrefix(): string {
    return `${this.namespace}/`;
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
    // The images are downloaded once, copied to each worker
    const sprites: Sprite[] = patterns && style.sprite ? await this.loadSprites(style.sprite) : [];
    if (this.destroyed) throw new DOMException('isard: vector tiles destroyed', 'AbortError');
    this.pool.broadcast({ type: 'init', key, tiles, maxzoom, layers, rasters, ...(sprites.length ? { sprites } : {}) });
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
    this.ground ??= layer;
    return layer;
  }

  /**
   * A symbol layer with the labels of the style's symbol layers (or those `layers` keeps), updated as the view
   * moves: the labels of the vector tiles in view, the higher style layers placed first. With a raster layer of
   * the style (`raster`), the tiles follow the levels the ground is drawn at: a tilted view takes coarser tiles far
   * away, with the few labels of a lower zoom, as near the camera it takes the fine ones. Without, the tiles around
   * the center at the zoom of the view. It stays above the layers added after it unless one is moved over it.
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
      placement: (p) => (p.textAlong ? 'line' : 'point'),
      textOptional: (p) => p.textOptional === true,
      icon: (p) => (p.icon ? this.iconPrefix + p.icon.image : ''),
      iconSize: (p) => size(p.icon?.size ?? 1, 1),
      iconAnchor: (p) => (p.icon?.anchor ?? 'center') as Anchor,
      iconOffset: (p) => p.icon?.offset ?? [0, 0],
      iconRotate: (p) => p.icon?.rotate ?? 0,
      iconRotationAlignment: (p) => p.icon?.alignment ?? 'viewport',
      iconOverlap: (p) => p.icon?.overlap ?? false,
      iconOptional: (p) => p.icon?.optional ?? false,
      iconTextFit: (p) => p.icon?.fit ?? 'none',
      iconTextFitPadding: (p) => p.icon?.padding ?? [0, 0, 0, 0],
      iconColor: (p) => p.icon?.color ?? '#000',
      iconOpacity: (p) => p.icon?.opacity ?? 1,
    });
    this.map.keepOnTop(symbol);
    this.layers.push(symbol);
    const renderer = this.ready.then(async ({ style }) => {
      // The style's fonts from its own glyphs
      if (style.glyphs) this.map.glyphs.register(style.glyphs, VectorTiles.fonts(style));
      // Its icons from its sprites, before the first labels
      const icons = style.layers.some((l) => l.type === 'symbol' && l.layout?.['icon-image'] !== undefined);
      if (icons && style.sprite) {
        for (const sprite of await this.loadSprites(style.sprite)) {
          if (!this.destroyed) this.map.icons.addSprite(this.iconPrefix, sprite.icons, () => readPixels(sprite.image));
        }
      }
      return this.renderer((l) => l.type === 'symbol' && layers(l), []);
    });
    renderer.catch(() => undefined);
    /** The labels of the tiles, as features by style layer, the higher layers first: each list is given to the symbol
     * layer as a group, shaped once while the tile is kept */
    const tiles = new Lru<string, Promise<TileLabels>>(256);
    let generation = 0;
    /** The tiles whose labels are shown */
    let shown = '';
    /** The update asked again after tiles failed */
    let retry = 0;
    const update = async () => {
      if (this.destroyed) return;
      const { key, maxzoom } = await renderer;
      const run = ++generation;
      const camera = this.map.getCamera();
      const zoom = Math.max(0, Math.min(Math.floor(camera.zoom), 24));
      const ground = this.ground && this.map.hasLayer(this.ground) ? this.map.engine.wantedTiles(this.ground.id) : [];
      const keys: string[] = [];
      if (ground.length > 0) {
        // The ground's tiles, the nearest first: a tile so many levels coarser than the finest in view has its
        // labels of a zoom as many under the view's, past the source's deepest level those of that level at the zoom
        let top = 0;
        for (let i = 0; i < ground.length; i += 3) top = Math.max(top, ground[i] as number);
        for (let i = 0; i < ground.length && keys.length < maxTiles; i += 3) {
          const [z, x, y] = [ground[i], ground[i + 1], ground[i + 2]] as [number, number, number];
          const at = Math.max(0, zoom - (top - z));
          const tz = Math.min(at, maxzoom, z);
          const shift = 2 ** (z - tz);
          const key = `${tz}/${Math.floor(x / shift)}/${Math.floor(y / shift)}${at > tz ? `@${at}` : ''}`;
          if (!keys.includes(key)) keys.push(key);
        }
      } else {
        const z = Math.min(zoom, maxzoom);
        const n = 2 ** z;
        const [west, south, east, north] = this.map.getBounds();
        const tx = (lon: number) => Math.floor(mercatorX(lon) * n);
        const ty = (lat: number) => Math.floor(mercatorY(lat) * n);
        const [cx, cy] = [mercatorX(camera.center[0]) * n, ty(camera.center[1])];
        // The tiles of the bounds, within a window around the center: a tilted view's bounds reach far, and only
        // the nearest `maxTiles` are kept anyway
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
        // Past the source's deepest level, its tiles' labels at the zoom of the view (the layers of the deeper zooms)
        for (const [x, y] of wanted.slice(0, maxTiles)) keys.push(`${z}/${x}/${y}${zoom > z ? `@${zoom}` : ''}`);
      }
      // The same tiles as the labels shown: nothing to do
      const set = keys.join(' ');
      if (set === shown) return;
      const lists = await Promise.all(
        keys.map((k, index) => {
          let p = tiles.get(k);
          if (p) return p;
          const [tile, deeper] = k.split('@');
          const [tz, x, y] = (tile ?? '').split('/').map(Number) as [number, number, number];
          // The nearest tiles first: the others wait for the raster tiles
          const priority: Priority = index < 4 ? 'auto' : 'low';
          const work: BasemapWork = {
            type: 'labels',
            key,
            z: tz,
            x,
            y,
            priority,
            ...(deeper === undefined ? {} : { zoom: Number(deeper) }),
          };
          p = this.request(tz, x, y, work).then(
            (r) => tileLabels('labels' in r ? r.labels : []),
            () => {
              // A network error is not kept: the tile is asked for again on the next update
              tiles.delete(k);
              return [];
            },
          );
          // A bounded cache of tiles' labels, the least recently used dropped
          tiles.set(k, p);
          return p;
        }),
      );
      if (run !== generation || this.destroyed) return;
      // A tile that failed is missing: the next update asks for it again, after a pause if no move brings one
      shown = keys.every((k) => tiles.has(k)) ? set : '';
      if (shown === '' && !retry) {
        retry = window.setTimeout(() => {
          retry = 0;
          refresh();
        }, RETRY_LABELS_MS);
      }
      // The labels of the higher style layers first: they win the collisions; for each style layer, the tiles nearest
      // to the center first
      const layerIds = [...new Set(lists.flatMap((list) => list.map(([layer]) => layer)))].sort((a, b) => b - a);
      const groups: Feature<LabelCandidate>[][] = [];
      for (const layer of layerIds) {
        for (const list of lists) {
          const group = list.find(([l]) => l === layer)?.[1];
          if (group) groups.push(group);
        }
      }
      symbol.setGroups(groups);
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
    this.cleanup.push(this.map.on('move', schedule), this.map.on('moveend', refresh), () => {
      clearTimeout(timer);
      clearTimeout(retry);
    });
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
    // The page's copy of the sprites (the workers have theirs), and their icons
    this.sprites
      ?.then((sprites) => {
        for (const sprite of sprites) sprite.image.close();
        this.map.icons.remove(this.iconPrefix, true);
      })
      .catch(() => undefined);
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

/** Colors of the layers of `basicStyle`, in turn */
const BASIC_COLORS = ['#2f6fd1', '#d1495b', '#2a9d8f', '#e9a03b', '#7b5ea7', '#5c7a29', '#c8553d', '#3d8eb9'];

/**
 * A plain style for vector tiles known only by their layers' names (a PMTiles archive's `vector_layers`, an MBTiles'
 * metadata): each layer's polygons filled, its lines and outlines stroked and its points as circles, a color per
 * layer, the `name` of points as labels. `source` is the style's vector source (`url`: TileJSON or `pmtiles://…`, or
 * `tiles`)
 */
export function basicStyle(
  source: { url?: string; tiles?: string[]; minzoom?: number; maxzoom?: number },
  layers: readonly string[],
): Style {
  const out: StyleLayer[] = [];
  layers.forEach((name, i) => {
    const color = BASIC_COLORS[i % BASIC_COLORS.length] as string;
    const base = { source: 'data', 'source-layer': name };
    out.push(
      { id: `${name}-fill`, type: 'fill', ...base, paint: { 'fill-color': color, 'fill-opacity': 0.18 } },
      { id: `${name}-line`, type: 'line', ...base, paint: { 'line-color': color, 'line-width': 1.2 } },
      {
        id: `${name}-circle`,
        type: 'circle',
        ...base,
        paint: { 'circle-color': color, 'circle-radius': 3.5, 'circle-stroke-color': '#fff', 'circle-stroke-width': 1 },
      },
    );
  });
  layers.forEach((name) => {
    out.push({
      id: `${name}-label`,
      type: 'symbol',
      source: 'data',
      'source-layer': name,
      filter: ['all', ['==', ['geometry-type'], 'Point'], ['has', 'name']],
      layout: { 'text-field': ['get', 'name'], 'text-size': 12, 'text-offset': [0, 0.9], 'text-anchor': 'top' },
      paint: { 'text-color': '#1d2433', 'text-halo-color': '#fff', 'text-halo-width': 1.5 },
    });
  });
  return { version: 8, sources: { data: { type: 'vector', ...source } }, layers: out };
}

/** Options of `GeoMap.addPmtiles` */
export interface PmtilesOptions {
  /** What the archive holds (default from its header: raster images or vector tiles) */
  kind?: 'raster' | 'vector';
  /** A MapLibre style for vector tiles (its vector source `pmtiles://…`); default `basicStyle` of the archive's layers */
  style?: Style;
  opacity?: number;
  /** Texture size of raster tiles (default 256; 512 for the sharper tiles of a 512 px archive) */
  size?: number;
  /** Credits, HTML (default the archive's `attribution` metadata, as text) */
  attribution?: string;
  before?: Layer;
}

/** HTML of a file reduced to its text, escaped: credits from an archive's metadata cannot run in the page */
function plainText(html: string): string {
  return html
    .replace(/<[^>]*>/g, '')
    .replace(/&(?!(amp|lt|gt|quot|copy|#\d+);)/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');
}

/** @internal `GeoMap.addPmtiles`: a raster layer of the archive's images, or vector tiles with their raster layer and
 * labels */
export async function pmtilesLayer(
  map: GeoMap,
  url: string,
  options: PmtilesOptions,
): Promise<RasterLayer | VectorTiles> {
  const archiveUrl = pmtilesUrl(url.startsWith('pmtiles://') ? url : `pmtiles://${url}`) as string;
  const archive = pmtiles(archiveUrl);
  const [header, metadata] = await Promise.all([archive.header(), archive.metadata().catch(() => ({}))]);
  const credit = (metadata as { attribution?: unknown }).attribution;
  const attribution = options.attribution ?? (typeof credit === 'string' && credit ? plainText(credit) : undefined);
  const kind = options.kind ?? (header.tileType === 'mvt' ? 'vector' : 'raster');
  const credits = attribution ? { attribution } : {};
  if (kind === 'raster') {
    return map.addRaster({
      url: `pmtiles://${archiveUrl}`,
      minZoom: header.minZoom,
      maxZoom: Math.max(header.maxZoom, 19),
      ...(options.opacity === undefined ? {} : { opacity: options.opacity }),
      ...(options.size === undefined ? {} : { size: options.size }),
      ...(options.before ? { before: options.before } : {}),
      ...credits,
    });
  }
  const layers = (metadata as { vector_layers?: { id?: unknown }[] }).vector_layers;
  const names = Array.isArray(layers) ? layers.map((l) => l?.id).filter((id) => typeof id === 'string') : [];
  const style = options.style ?? basicStyle({ url: `pmtiles://${archiveUrl}` }, names);
  const tiles = map.vectorTiles({ style, ...credits });
  tiles.raster({
    ...(options.opacity === undefined ? {} : { opacity: options.opacity }),
    ...(options.before ? { before: options.before } : {}),
  });
  tiles.labels();
  return tiles;
}

/** A raster loader reading the tiles of a `pmtiles://` archive URL (images of any size, drawn at `size`); undefined
 * for another URL */
export function pmtilesRasterLoader(url: string, size: number): RasterOptions['loader'] {
  const archiveUrl = pmtilesUrl(url);
  if (!archiveUrl) return undefined;
  const archive = pmtiles(archiveUrl);
  return async (z, x, y, signal) => {
    const bytes = await archive.tile(z, x, y, signal);
    if (!bytes) return null;
    return createImageBitmap(new Blob([bytes as Uint8Array<ArrayBuffer>]), { resizeWidth: size, resizeHeight: size });
  };
}
