/**
 * Pool of decoding workers for 3D Tiles content and point clouds. The engine calls `load`, `loadPoints` and `abort`
 * (`TileLoaderLike`); the main thread then only uploads the decoded arrays to the GPU.
 */

import type { DecodedTile, TextureFormats, TileLoaderLike } from '../wasm/isard.js';
import type { DecoderUrls } from './decoders.ts';
import type { WorkerReply, WorkerRequest } from './protocol.ts';
import { headerRules, onHeaderRules } from './requests.ts';

interface Pending {
  resolve: (result: DecodedTile | Uint8Array<ArrayBuffer>) => void;
  reject: (error: Error) => void;
  worker: Worker;
}

export class TileLoader implements TileLoaderLike {
  private workers: Worker[] = [];
  /** Requests in flight per worker */
  private readonly busy = new Map<Worker, number>();
  private readonly pending = new Map<number, Pending>();
  /** Every worker failed: why */
  private failed: Error | undefined;
  /** The page's request headers, sent to the workers as they change */
  private readonly unfollow = onHeaderRules((rules) => {
    for (const worker of this.workers) this.send(worker, { type: 'headers', rules });
  });

  constructor(
    private readonly module: WebAssembly.Module,
    private readonly workerUrl: string | URL,
    private readonly size: number,
    private readonly decoders: DecoderUrls,
    private readonly formats: TextureFormats,
    /** URL of the point cloud module, for the workers */
    private readonly points: string = '',
  ) {}

  /** Called by the engine: fetch and decode off the main thread, on the least busy worker */
  load(
    id: number,
    url: string,
    transform: Float64Array,
    yUp: boolean,
    reload: boolean,
    priority?: 'high' | 'low',
  ): Promise<DecodedTile> {
    if (this.failed) return Promise.reject(this.failed);
    const worker = this.leastBusy();
    this.busy.set(worker, (this.busy.get(worker) ?? 0) + 1);
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve: resolve as Pending['resolve'], reject, worker });
      this.send(worker, { type: 'load', id, url, transform, yUp, reload, ...(priority ? { priority } : {}) });
    });
  }

  /** Called by the engine: the points of a COPC node, fetched (a range request) and decoded off the main thread */
  loadPoints(
    id: number,
    url: string,
    offset: number,
    size: number,
    laz: Uint8Array,
    layout: Float64Array,
    grid: Float64Array,
    count: number,
    priority?: 'high' | 'low',
  ): Promise<Uint8Array<ArrayBuffer>> {
    if (this.failed) return Promise.reject(this.failed);
    const worker = this.leastBusy();
    this.busy.set(worker, (this.busy.get(worker) ?? 0) + 1);
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve: resolve as Pending['resolve'], reject, worker });
      const message = { type: 'points', id, url, offset, size, laz, layout, grid, count } as const;
      this.send(worker, { ...message, ...(priority ? { priority } : {}) });
    });
  }

  abort(id: number): void {
    const pending = this.pending.get(id);
    if (pending) this.send(pending.worker, { type: 'abort', id });
  }

  destroy(): void {
    this.unfollow();
    for (const worker of this.workers) worker.terminate();
    this.workers = [];
    this.busy.clear();
    for (const { reject } of this.pending.values()) reject(new Error('loader destroyed'));
    this.pending.clear();
  }

  /** Workers start with the first tile: a map without 3D Tiles costs nothing */
  private leastBusy(): Worker {
    if (this.workers.length === 0) {
      for (let i = 0; i < this.size; i++) {
        const worker = new Worker(this.workerUrl, { type: 'module' });
        worker.onmessage = (event: MessageEvent<WorkerReply>) => this.receive(worker, event.data);
        // A worker that fails to load (a bundler serving it from elsewhere) or to read a message fails its requests
        worker.onerror = (event) => {
          event.preventDefault();
          this.fail(worker, `the decoding worker ${this.workerUrl} failed${event.message ? `: ${event.message}` : ''}`);
        };
        worker.onmessageerror = () =>
          this.fail(worker, `the decoding worker ${this.workerUrl} sent an unreadable message`);
        const { module, decoders, formats, points } = this;
        this.send(worker, { type: 'init', module, decoders, formats, points });
        const rules = headerRules();
        if (rules.length > 0) this.send(worker, { type: 'headers', rules });
        this.workers.push(worker);
        this.busy.set(worker, 0);
      }
    }
    let best = this.workers[0] as Worker;
    for (const worker of this.workers) {
      if ((this.busy.get(worker) ?? 0) < (this.busy.get(best) ?? 0)) best = worker;
    }
    return best;
  }

  private send(worker: Worker, message: WorkerRequest): void {
    worker.postMessage(message);
  }

  private receive(worker: Worker, reply: WorkerReply): void {
    this.busy.set(worker, Math.max(0, (this.busy.get(worker) ?? 1) - 1));
    const pending = this.pending.get(reply.id);
    if (!pending) return;
    this.pending.delete(reply.id);
    if (reply.type === 'tile') pending.resolve(reply.tile);
    else if (reply.type === 'points') pending.resolve(reply.vertices);
    else if (reply.aborted) pending.reject(new DOMException(`tile ${reply.id} aborted`, 'AbortError'));
    else pending.reject(new Error(reply.error));
  }

  /** Rejects the requests of a failed worker */
  private fail(worker: Worker, message: string): void {
    const error = new Error(
      `isard: ${message}. If a bundler moves the worker, give its URL in the \`workerUrl\` option (see "With a bundler" in the guide)`,
    );
    for (const [id, pending] of this.pending) {
      if (pending.worker !== worker) continue;
      this.pending.delete(id);
      pending.reject(error);
    }
    // Out of the pool: when none is left, the next requests fail at once
    worker.terminate();
    this.workers = this.workers.filter((w) => w !== worker);
    this.busy.delete(worker);
    if (this.workers.length === 0) this.failed = error;
  }
}
