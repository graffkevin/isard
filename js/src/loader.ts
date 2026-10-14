/**
 * Pool of decoding workers for 3D Tiles content. The engine calls `load` and `abort`; the main thread then only
 * uploads the decoded arrays to the GPU.
 */

export interface DecodedTile {
  anchor: Float64Array;
  primitives: { vertices: Float32Array; indices: Uint32Array; bitmap: ImageBitmap | null }[];
}

interface Pending {
  resolve: (tile: DecodedTile) => void;
  reject: (error: Error) => void;
  worker: Worker;
}

export class TileLoader {
  private workers: Worker[] = [];
  private readonly load_: Map<Worker, number> = new Map();
  private readonly pending = new Map<number, Pending>();

  constructor(
    private readonly module: WebAssembly.Module,
    private readonly workerUrl: string | URL,
    private readonly size: number,
  ) {}

  /** Workers start on the first tile: a map without 3D Tiles costs nothing */
  private start(): void {
    for (let i = 0; i < this.size; i++) {
      const worker = new Worker(this.workerUrl, { type: 'module' });
      worker.onmessage = (e) => this.receive(worker, e.data);
      worker.postMessage({ init: this.module });
      this.workers.push(worker);
      this.load_.set(worker, 0);
    }
  }

  /** Called by the engine: fetch and decode off the main thread, least busy worker first */
  load(id: number, url: string, transform: Float64Array, yUp: boolean): Promise<DecodedTile> {
    if (this.workers.length === 0) this.start();
    const worker = this.workers.reduce((a, b) => ((this.load_.get(a) ?? 0) <= (this.load_.get(b) ?? 0) ? a : b));
    this.load_.set(worker, (this.load_.get(worker) ?? 0) + 1);
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject, worker });
      worker.postMessage({ id, url, transform, yUp });
    });
  }

  abort(id: number): void {
    this.pending.get(id)?.worker.postMessage({ abort: id });
  }

  private receive(worker: Worker, message: { id: number; tile?: DecodedTile; error?: string }): void {
    this.load_.set(worker, Math.max(0, (this.load_.get(worker) ?? 1) - 1));
    const pending = this.pending.get(message.id);
    if (!pending) return;
    this.pending.delete(message.id);
    if (message.tile) pending.resolve(message.tile);
    else pending.reject(new Error(message.error ?? 'decoding failed'));
  }

  destroy(): void {
    for (const worker of this.workers) worker.terminate();
    this.workers = [];
  }
}
