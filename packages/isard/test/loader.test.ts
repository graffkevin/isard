import { afterAll, beforeEach, describe, expect, test } from 'bun:test';
import { TileLoader } from '../src/loader.ts';
import type { WorkerReply, WorkerRequest } from '../src/protocol.ts';
import type { DecodedTile } from '../src/wasm/isard.js';
import { setGlobals } from './fakes.ts';

/** A worker stand-in that keeps the messages it gets */
class FakeWorker {
  static all: FakeWorker[] = [];
  messages: WorkerRequest[] = [];
  terminated = false;
  onmessage: ((event: MessageEvent<WorkerReply>) => void) | null = null;
  onerror: ((event: ErrorEvent) => void) | null = null;
  onmessageerror: (() => void) | null = null;
  constructor(
    readonly url: string | URL,
    readonly options: WorkerOptions,
  ) {
    FakeWorker.all.push(this);
  }
  postMessage(message: WorkerRequest) {
    this.messages.push(message);
  }
  terminate() {
    this.terminated = true;
  }
  reply(data: WorkerReply) {
    this.onmessage?.({ data } as MessageEvent<WorkerReply>);
  }
  /** Fails to load; returns whether the default handling was prevented */
  crash(message = 'boom'): boolean {
    let prevented = false;
    this.onerror?.({
      message,
      preventDefault() {
        prevented = true;
      },
    } as ErrorEvent);
    return prevented;
  }
  /** The ids of the tiles it was asked for */
  loads(): number[] {
    return this.messages.flatMap((m) => (m.type === 'load' ? [m.id] : []));
  }
}

const restore = setGlobals({ Worker: FakeWorker });
afterAll(restore);

const module = {} as WebAssembly.Module;
const decoders = { draco: 'd.js', basis: 'b.js' } as never;
const formats = { astc: true, bptc: false, etc: false, s3tc: true };
const transform = Float64Array.of(1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1);
const tile = { anchor: [1, 2, 3], primitives: [] } as unknown as DecodedTile;

const loader = (size = 2) => new TileLoader(module, 'decoder.js', size, decoders, formats, 'points.wasm');
/** Keeps a request whose outcome the test ignores from failing it as unhandled */
const quiet = (p: Promise<unknown>) => void p.catch(() => {});
const worker = (i: number) => FakeWorker.all[i] as FakeWorker;

beforeEach(() => {
  FakeWorker.all = [];
});

describe('workers', () => {
  test('start with the first tile, each initialized with the module, decoders and texture formats', () => {
    const l = loader(3);
    expect(FakeWorker.all.length).toBe(0);
    quiet(l.load(1, 'a.glb', transform, true, false));
    expect(FakeWorker.all.length).toBe(3);
    for (const w of FakeWorker.all) {
      expect(w.url).toBe('decoder.js');
      expect(w.options).toEqual({ type: 'module' });
      expect(w.messages[0]).toEqual({ type: 'init', module, decoders, formats, points: 'points.wasm' });
    }
    quiet(l.load(2, 'b.glb', transform, true, false));
    expect(FakeWorker.all.length).toBe(3);
    l.destroy();
  });

  test('a load message carries the request; the priority only when given', () => {
    const l = loader(1);
    quiet(l.load(7, 'a.glb', transform, false, true, 'high'));
    quiet(l.load(8, 'b.glb', transform, true, false, 'low'));
    quiet(l.load(9, 'c.glb', transform, true, false));
    const [, high, low, none] = worker(0).messages;
    expect(high).toEqual({ type: 'load', id: 7, url: 'a.glb', transform, yUp: false, reload: true, priority: 'high' });
    expect(low).toMatchObject({ id: 8, priority: 'low', reload: false, yUp: true });
    expect(none).toEqual({ type: 'load', id: 9, url: 'c.glb', transform, yUp: true, reload: false });
    expect(none && 'priority' in none).toBe(false);
    l.destroy();
  });

  test('requests go to the least busy worker; an answer frees its worker', () => {
    const l = loader(2);
    for (const id of [1, 2, 3]) quiet(l.load(id, `${id}.glb`, transform, true, false));
    expect(worker(0).loads()).toEqual([1, 3]);
    expect(worker(1).loads()).toEqual([2]);
    // Worker 0 answers both: it is now the least busy
    worker(0).reply({ type: 'tile', id: 1, tile });
    worker(0).reply({ type: 'tile', id: 3, tile });
    quiet(l.load(4, '4.glb', transform, true, false));
    expect(worker(0).loads()).toEqual([1, 3, 4]);
    l.destroy();
  });
});

describe('answers', () => {
  test('a tile resolves its request, an error rejects it', async () => {
    const l = loader(1);
    const a = l.load(1, 'a.glb', transform, true, false);
    const b = l.load(2, 'b.glb', transform, true, false);
    worker(0).reply({ type: 'error', id: 2, error: 'HTTP 404', aborted: false });
    worker(0).reply({ type: 'tile', id: 1, tile });
    expect(await a).toBe(tile);
    await expect(b).rejects.toThrow('HTTP 404');
    l.destroy();
  });

  test('an aborted tile rejects with an AbortError', async () => {
    const l = loader(1);
    const a = l.load(5, 'a.glb', transform, true, false);
    worker(0).reply({ type: 'error', id: 5, error: 'AbortError', aborted: true });
    const error = await a.catch((e: unknown) => e);
    expect(error).toBeInstanceOf(DOMException);
    expect((error as DOMException).name).toBe('AbortError');
    l.destroy();
  });

  test('answers to unknown or settled requests are ignored', async () => {
    const l = loader(1);
    const a = l.load(1, 'a.glb', transform, true, false);
    worker(0).reply({ type: 'tile', id: 99, tile });
    worker(0).reply({ type: 'tile', id: 1, tile });
    worker(0).reply({ type: 'error', id: 1, error: 'late', aborted: false });
    expect(await a).toBe(tile);
    l.destroy();
  });
});

describe('abort', () => {
  test('goes to the worker loading the tile, only while it is pending', () => {
    const l = loader(2);
    quiet(l.load(1, 'a.glb', transform, true, false));
    quiet(l.load(2, 'b.glb', transform, true, false));
    l.abort(2);
    expect(worker(1).messages.at(-1)).toEqual({ type: 'abort', id: 2 });
    expect(worker(0).messages.some((m) => m.type === 'abort')).toBe(false);
    // Done or never asked: nothing sent
    worker(0).reply({ type: 'tile', id: 1, tile });
    const before = worker(0).messages.length;
    l.abort(1);
    l.abort(42);
    expect(worker(0).messages.length).toBe(before);
    l.destroy();
  });
});

describe('failures', () => {
  test('a worker that fails rejects its requests only, with a hint, and leaves the pool', async () => {
    const l = loader(2);
    const a = l.load(1, 'a.glb', transform, true, false);
    const b = l.load(2, 'b.glb', transform, true, false);
    expect(worker(0).crash('404')).toBe(true);
    const error = (await a.catch((e: unknown) => e)) as Error;
    expect(error.message).toContain('the decoding worker decoder.js failed: 404');
    expect(error.message).toContain('workerUrl');
    expect(worker(0).terminated).toBe(true);
    // The other one goes on, and takes the next requests
    worker(1).reply({ type: 'tile', id: 2, tile });
    expect(await b).toBe(tile);
    quiet(l.load(3, 'c.glb', transform, true, false));
    expect(worker(1).loads()).toEqual([2, 3]);
    expect(worker(0).loads()).toEqual([1]);
    l.destroy();
  });

  test('an unreadable message fails the worker too', async () => {
    const l = loader(1);
    const a = l.load(1, 'a.glb', transform, true, false);
    worker(0).onmessageerror?.();
    await expect(a).rejects.toThrow('sent an unreadable message');
    l.destroy();
  });

  test('with every worker failed, requests fail at once and no worker restarts', async () => {
    const l = loader(1);
    const a = l.load(1, 'a.glb', transform, true, false);
    worker(0).crash('');
    const first = (await a.catch((e: unknown) => e)) as Error;
    expect(first.message).toContain('the decoding worker decoder.js failed.');
    const again = await l.load(2, 'b.glb', transform, true, false).catch((e: unknown) => e);
    expect(again).toBe(first);
    expect(FakeWorker.all.length).toBe(1);
  });
});

describe('point clouds', () => {
  test('a COPC chunk goes to a worker with what decodes it, and resolves with its vertices', async () => {
    const l = loader(1);
    const laz = new Uint8Array([1]);
    const layout = new Float64Array(8);
    const grid = new Float64Array(30);
    const done = l.loadPoints(5, 'c.copc.laz', 100, 20, laz, layout, grid, 7, 'low');
    expect(worker(0).messages[1]).toEqual({
      type: 'points',
      id: 5,
      url: 'c.copc.laz',
      offset: 100,
      size: 20,
      laz,
      layout,
      grid,
      count: 7,
      priority: 'low',
    });
    const vertices = new Uint8Array(140);
    worker(0).reply({ type: 'points', id: 5, vertices });
    expect(await done).toBe(vertices);
    l.destroy();
  });
});

describe('destroy', () => {
  test('terminates the workers and rejects the pending requests', async () => {
    const l = loader(2);
    const a = l.load(1, 'a.glb', transform, true, false);
    const b = l.load(2, 'b.glb', transform, true, false);
    l.destroy();
    await expect(a).rejects.toThrow('loader destroyed');
    await expect(b).rejects.toThrow('loader destroyed');
    expect(FakeWorker.all.every((w) => w.terminated)).toBe(true);
    // An answer arriving after is ignored
    worker(0).reply({ type: 'tile', id: 1, tile });
  });
});
