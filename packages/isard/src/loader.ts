/**
 * Pool of decoding workers for 3D Tiles content. The engine calls `load` and `abort` (`TileLoaderLike`); the main
 * thread then only uploads the decoded arrays to the GPU.
 */
import type { DecoderUrls } from './decoders.ts';
import type { WorkerReply, WorkerRequest } from './protocol.ts';
import type { DecodedTile, TextureFormats, TileLoaderLike } from './wasm/isard.js';

interface Pending {
  resolve: (tile: DecodedTile) => void;
  reject: (error: Error) => void;
  worker: Worker;
}

export class TileLoader implements TileLoaderLike {
  private workers: Worker[] = [];
  /** Requests in flight per worker */
  private readonly busy = new Map<Worker, number>();
  private readonly pending = new Map<number, Pending>();

  constructor(
    private readonly module: WebAssembly.Module,
    private readonly workerUrl: string | URL,
    private readonly size: number,
    private readonly decoders: DecoderUrls,
    private readonly formats: TextureFormats,
  ) {}

  /** Called by the engine: fetch and decode off the main thread, on the least busy worker */
  load(id: number, url: string, transform: Float64Array, yUp: boolean, reload: boolean): Promise<DecodedTile> {
    const worker = this.leastBusy();
    this.busy.set(worker, (this.busy.get(worker) ?? 0) + 1);
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject, worker });
      this.send(worker, { type: 'load', id, url, transform, yUp, reload });
    });
  }

  abort(id: number): void {
    const pending = this.pending.get(id);
    if (pending) this.send(pending.worker, { type: 'abort', id });
  }

  destroy(): void {
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
        this.send(worker, { type: 'init', module: this.module, decoders: this.decoders, formats: this.formats });
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
    else pending.reject(new Error(reply.error));
  }
}
