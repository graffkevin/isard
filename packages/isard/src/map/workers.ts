/**
 * A map's workers: the pool drawing vector tiles, shared by the map's `VectorTiles`, which also decodes the relief and
 * reprojects rasters off the page; and the loaders of the relief, one per tile source.
 */

import type { RemoteReprojection } from '../basemap/grid.ts';
import { encodedTerrainLoader, type PngEncoding, type RemoteHeights } from '../basemap/terrain.ts';
import { BasemapWorkers, defaultWorkerCount } from '../basemap/vectortiles.ts';
import type { TerrainLoader } from '../settings.ts';
import type { GeoMapOptions } from '../types.ts';

/** The workers of a map, created on first use */
export class MapWorkers {
  /** URL of the workers' script */
  readonly url: string | URL;
  private readonly workers: GeoMapOptions['workers'];
  /** The map is destroyed: nothing more is sent to the workers */
  private readonly gone: () => boolean;
  /** Whose requests the workers' answer (cancelled together) */
  private readonly owner: object;
  /** The shared pool, created on first use */
  private shared: BasemapWorkers | undefined;
  /** The loaders of the relief by encoding, URL and floor: the same loader for the same tiles (the engine keeps its
   * tiles when only the exaggeration changes) */
  private readonly terrainLoaders = new Map<string, TerrainLoader>();

  /** `workers`: the map's option (false or 0, no worker); `owner`: the map */
  constructor(url: string | URL, workers: GeoMapOptions['workers'], gone: () => boolean, owner: object) {
    this.url = url;
    this.workers = workers;
    this.gone = gone;
    this.owner = owner;
  }

  /**
   * The workers for a `VectorTiles`: the shared pool, or a pool of its own (`own`, to stop with it) when it asks for
   * another count or URL
   */
  pool(count?: number, url?: string | URL): { pool: BasemapWorkers; own: boolean } {
    const shared = defaultWorkerCount();
    const workers = Math.max(1, count ?? shared);
    const workerUrl = url ?? this.url;

    if (workers === shared && String(workerUrl) === String(this.url)) {
      this.shared ??= new BasemapWorkers(shared, this.url);

      return { pool: this.shared, own: false };
    }

    return { pool: new BasemapWorkers(workers, workerUrl), own: true };
  }

  /** Stops the shared pool */
  terminate(): void {
    this.shared?.terminate();
    this.shared = undefined;
  }

  /** Rasters reprojected by the workers (in the page without them) */
  readonly remoteReprojection: RemoteReprojection = (job, tile, signal) =>
    this.decodingPool()?.reproject(job, tile, signal, this.owner);

  /** The loader of the relief tiles of `url` in a PNG encoding (one per URL, encoding and floor) */
  readonly terrainLoader = (url: string, encoding: PngEncoding, floor?: number): TerrainLoader => {
    const key = `${encoding} ${floor ?? ''} ${url}`;
    const cached = this.terrainLoaders.get(key);

    if (cached) return cached;

    const loader = encodedTerrainLoader(url, encoding, floor, this.remoteHeights);

    this.terrainLoaders.set(key, loader);

    return loader;
  };

  private readonly remoteHeights: RemoteHeights = (url, encoding, floor, tile, signal) =>
    this.decodingPool()?.heights(url, encoding, floor, tile, signal, this.owner);

  /**
   * The workers decoding the relief and reprojecting rasters: the shared pool, unless the map has no workers
   * (`workers: false` or 0, or none in this browser): that is done in the page then
   */
  private decodingPool(): BasemapWorkers | undefined {
    const workers = this.workers;

    if (this.gone() || workers === false || (workers !== undefined && workers <= 0)) return undefined;
    if (typeof Worker === 'undefined' || typeof OffscreenCanvas === 'undefined') return undefined;

    return this.pool().pool;
  }
}
