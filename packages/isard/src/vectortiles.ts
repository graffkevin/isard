/**
 * Vector basemaps from a MapLibre style (the Géoplateforme's PLAN.IGN styles by default): the fills, lines and circles
 * drawn tile by tile in workers into the raster pool, so that they lie on the relief like any basemap and composite
 * with the hillshade in one pass; and the labels of the style's symbol layers in a symbol layer of their own, kept
 * above the application's layers (as MapLibre's `beforeId: firstSymbol` does), shaped and placed by the engine.
 *
 * Any subset of the style can be drawn on its own raster layer (`raster({ layers })`), at its own place in the stack:
 * rivers from the basemap's data over a flood layer, for instance, restyled with `extraLayers`.
 */
import type { BasemapRequest, BasemapResponse, LabelCandidate, Sprite } from './basemap.worker.ts';
import type { Feature, Size } from './features.ts';
import type { RasterLayer } from './layers.ts';
import type { GeoMap } from './map.ts';
import type { Anchor } from './overlay.ts';
import type { SymbolLayer } from './symbols.ts';
import type { Json, Style, StyleLayer } from './vectorstyle.ts';

/** PLAN.IGN vector styles of the Géoplateforme */
export const PLAN_STYLES = {
  standard: 'https://data.geopf.fr/annexes/ressources/vectorTiles/styles/PLAN.IGN/standard.json',
  grey: 'https://data.geopf.fr/annexes/ressources/vectorTiles/styles/PLAN.IGN/gris.json',
} as const;

/** Vector basemaps by name (see `Basemap`) */
export const VECTOR_BASEMAPS = {
  'plan-vector': PLAN_STYLES.standard,
  'plan-grey-vector': PLAN_STYLES.grey,
} as const;

export interface VectorTilesOptions {
  /** A style URL or object (default the PLAN.IGN grey style) */
  style?: string | Style;
  /** Workers drawing the tiles (default cores - 1, 1 to 4) */
  workers?: number;
  /** Where to load the worker from, when a bundler does not resolve the default URL */
  workerUrl?: string | URL;
}

/** Text size of a style (number or legacy stops) as a size for the symbol layer */
function size(value: Json): Size {
  if (typeof value === 'number') return value;
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

export class VectorTiles {
  private readonly workers: Worker[];
  private readonly pending = new Map<number, { resolve: (r: BasemapResponse) => void; reject: (e: Error) => void }>();
  private next = 1;
  private keys = 0;
  private readonly cleanup: (() => void)[] = [];
  /** The style once loaded, with its source resolved */
  readonly ready: Promise<{ style: Style; tiles: string; maxzoom: number }>;

  constructor(
    private readonly map: GeoMap,
    options: VectorTilesOptions = {},
  ) {
    const count = options.workers ?? Math.min(4, Math.max(1, (navigator.hardwareConcurrency || 2) - 1));
    const url = options.workerUrl ?? new URL('./basemap.worker.js', import.meta.url);
    this.workers = Array.from({ length: Math.max(1, count) }, () => {
      const worker = new Worker(url, { type: 'module' });
      worker.onmessage = ({ data }: MessageEvent<BasemapResponse>) => {
        const p = this.pending.get(data.id);
        if (!p) return;
        this.pending.delete(data.id);
        if ('error' in data) p.reject(new Error(data.error));
        else p.resolve(data);
      };
      return worker;
    });
    this.ready = this.load(options.style ?? PLAN_STYLES.grey);
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
    // Only the layers of that source (and the background)
    style.layers = style.layers.filter((l) => l.type === 'background' || l.source === name);
    return { style, tiles: template, maxzoom: maxzoom ?? 14 };
  }

  /** The worker of a vector tile: always the same one, which keeps it decoded for every raster tile and every label
   * request drawn from it (one download per vector tile) */
  private worker(vz: number, vx: number, vy: number): Worker {
    const k = (Math.abs(vx * 73856093) ^ Math.abs(vy * 19349663) ^ vz) % this.workers.length;
    return this.workers[k] ?? (this.workers[0] as Worker);
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

  private request(worker: Worker, message: BasemapRequest & { id: number }, signal?: AbortSignal) {
    return new Promise<BasemapResponse>((resolve, reject) => {
      this.pending.set(message.id, { resolve, reject });
      signal?.addEventListener('abort', () => {
        worker.postMessage({ type: 'abort', id: message.id } satisfies BasemapRequest);
        this.pending.delete(message.id);
        reject(new DOMException('aborted', 'AbortError'));
      });
      worker.postMessage(message);
    });
  }

  /** Sends a renderer of these layers to every worker; returns its key */
  private async renderer(select: (layer: StyleLayer) => boolean, extra: StyleLayer[]) {
    const { style, tiles, maxzoom } = await this.ready;
    const key = `r${this.keys++}`;
    const layers = [...style.layers.filter(select), ...extra];
    const patterns = layers.some((l) => l.paint?.['fill-pattern'] !== undefined);
    const sprite = patterns && style.sprite ? await this.loadSprite(style.sprite) : undefined;
    for (const w of this.workers) {
      // The image is copied to each worker (structured clone), downloaded once
      w.postMessage({
        type: 'init',
        key,
        tiles,
        maxzoom,
        layers,
        ...(sprite ? { sprite } : {}),
      } satisfies BasemapRequest);
    }
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
    const loader = async (z: number, x: number, y: number, signal: AbortSignal) => {
      const { key, maxzoom } = await renderer;
      // The vector tile it is drawn from (see the worker)
      const vz = Math.max(0, Math.min(z - 1, maxzoom));
      const shift = z - vz;
      const response = await this.request(
        this.worker(vz, x >> shift, y >> shift),
        { type: 'render', key, id: this.next++, z, x, y, size: texture },
        signal,
      );
      return 'image' in response ? response.image : null;
    };
    return this.map.addRaster({
      url: '',
      size: texture,
      opacity,
      minZoom: options.minZoom ?? 0,
      maxZoom: options.maxZoom ?? 20,
      loader,
      ...(before ? { before } : {}),
    });
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
    const renderer = this.renderer((l) => l.type === 'symbol' && layers(l), []);
    const tiles = new Map<string, Promise<LabelCandidate[]>>();
    let generation = 0;
    const update = async () => {
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
      const lists = await Promise.all(
        keys.map((k) => {
          let p = tiles.get(k);
          if (!p) {
            const [tz, x, y] = k.split('/').map(Number) as [number, number, number];
            p = this.request(this.worker(tz, x, y), { type: 'labels', key, id: this.next++, z: tz, x, y }).then(
              (r) => ('labels' in r ? r.labels : []),
              () => [],
            );
            tiles.set(k, p);
            // A bounded cache of tiles' labels
            if (tiles.size > 256) {
              const oldest = tiles.keys().next().value;
              if (oldest !== undefined) tiles.delete(oldest);
            }
          }
          return p;
        }),
      );
      if (run !== generation) return;
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
    let timer = 0;
    const schedule = () => {
      if (timer) return;
      timer = window.setTimeout(() => {
        timer = 0;
        void update();
      }, 250);
    };
    this.cleanup.push(
      this.map.on('move', schedule),
      this.map.on('moveend', () => void update()),
    );
    void update();
    return symbol;
  }

  destroy(): void {
    for (const off of this.cleanup) off();
    for (const w of this.workers) w.terminate();
    for (const p of this.pending.values()) p.reject(new Error('destroyed'));
    this.pending.clear();
  }
}
