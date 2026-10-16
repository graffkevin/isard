import { describe, expect, test } from 'bun:test';
import type { BasemapRequest, BasemapResponse } from '../src/basemap.worker.ts';
import { BasemapWorkers, type PoolWorker, rendererKey, tilePriority, workerIndex } from '../src/vectortiles.ts';

/** A worker stand-in that keeps the messages it gets */
class FakeWorker {
  messages: BasemapRequest[] = [];
  terminated = false;
  onmessage: PoolWorker['onmessage'] = null;
  onerror: PoolWorker['onerror'] = null;
  onmessageerror: PoolWorker['onmessageerror'] = null;
  postMessage(message: BasemapRequest) {
    this.messages.push(message);
  }
  terminate() {
    this.terminated = true;
  }
  /** Answers a request */
  reply(data: BasemapResponse) {
    this.onmessage?.call(this as unknown as Worker, { data } as MessageEvent<BasemapResponse>);
  }
  /** Fails to load */
  crash() {
    this.onerror?.call(this as unknown as Worker, { message: 'boom', preventDefault() {} } as ErrorEvent);
  }
}

const pool = (count: number) => {
  const workers: FakeWorker[] = [];
  const p = new BasemapWorkers(count, 'worker.js', () => {
    const w = new FakeWorker();
    workers.push(w);
    return w as unknown as PoolWorker;
  });
  return { p, workers };
};

const work = { type: 'labels', key: 'k', z: 1, x: 0, y: 0 } as const;
const lastId = (w: FakeWorker) => {
  const m = w.messages.at(-1);
  return m && 'id' in m ? m.id : -1;
};

describe('pool keys and workers', () => {
  test('renderer keys are unique across the VectorTiles of a pool', () => {
    const { p } = pool(1);
    const [a, b] = [p.namespace(), p.namespace()];
    expect(a).not.toBe(b);
    expect(rendererKey(a, 0)).not.toBe(rendererKey(b, 0));
    expect(rendererKey(a, 0)).not.toBe(rendererKey(a, 1));
  });

  test('a vector tile always goes to the same worker, within the pool', () => {
    for (const [z, x, y] of [
      [3, 4, 5],
      [14, 8300, 5600],
      [0, 0, 0],
    ] as const) {
      const i = workerIndex(z, x, y, 3);
      expect(i).toBe(workerIndex(z, x, y, 3));
      expect(i).toBeGreaterThanOrEqual(0);
      expect(i).toBeLessThan(3);
    }
  });
});

describe('pool requests', () => {
  test('answers go to their request; ids are unique across owners', async () => {
    const { p, workers } = pool(1);
    const w = workers[0] as FakeWorker;
    const a = p.request(w as unknown as PoolWorker, work, {});
    const ida = lastId(w);
    const b = p.request(w as unknown as PoolWorker, work, {});
    const idb = lastId(w);
    expect(ida).not.toBe(idb);
    w.reply({ id: idb, labels: [] });
    w.reply({ id: ida, error: 'nope' });
    expect(await b).toEqual({ id: idb, labels: [] });
    await expect(a).rejects.toThrow('nope');
    expect(p.size).toBe(0);
  });

  test('cancel stops the requests of one owner only, and tells the worker', async () => {
    const { p, workers } = pool(1);
    const w = workers[0] as FakeWorker;
    const [mine, other] = [{}, {}];
    const a = p.request(w as unknown as PoolWorker, work, mine);
    const ida = lastId(w);
    const b = p.request(w as unknown as PoolWorker, work, other);
    const idb = lastId(w);
    p.cancel(mine);
    await expect(a).rejects.toThrow('destroyed');
    expect(w.messages).toContainEqual({ type: 'abort', id: ida });
    expect(w.terminated).toBe(false);
    w.reply({ id: idb, missing: true });
    expect(await b).toEqual({ id: idb, missing: true });
  });

  test('an abort signal rejects its request once', async () => {
    const { p, workers } = pool(1);
    const w = workers[0] as FakeWorker;
    const controller = new AbortController();
    const a = p.request(w as unknown as PoolWorker, work, {}, controller.signal);
    const id = lastId(w);
    controller.abort();
    await expect(a).rejects.toThrow('aborted');
    expect(w.messages).toContainEqual({ type: 'abort', id });
    // A late answer is ignored
    w.reply({ id, labels: [] });
    expect(p.size).toBe(0);
  });

  test('a failed worker fails its requests, not the others', async () => {
    const { p, workers } = pool(2);
    const [w0, w1] = workers as [FakeWorker, FakeWorker];
    const a = p.request(w0 as unknown as PoolWorker, work, {});
    const b = p.request(w1 as unknown as PoolWorker, work, {});
    const idb = lastId(w1);
    w0.crash();
    await expect(a).rejects.toThrow('basemapWorkerUrl');
    w1.reply({ id: idb, labels: [] });
    expect(await b).toEqual({ id: idb, labels: [] });
  });

  test('broadcast reaches every worker; terminate stops them and fails what is pending', async () => {
    const { p, workers } = pool(3);
    p.broadcast({ type: 'drop', key: 'v0:r0' });
    for (const w of workers) expect(w.messages).toEqual([{ type: 'drop', key: 'v0:r0' }]);
    const a = p.request(workers[0] as unknown as PoolWorker, work, {});
    p.terminate();
    await expect(a).rejects.toThrow('destroyed');
    for (const w of workers) expect(w.terminated).toBe(true);
    await expect(p.request(workers[0] as unknown as PoolWorker, work, {})).rejects.toThrow('destroyed');
  });
});

describe('tilePriority', () => {
  const camera = { center: [0, 0] as [number, number], zoom: 4 };
  test('high near the center, low far away or finer than the view', () => {
    // Level 5 tiles cover zoom 4; the center is at tile (16, 16)
    expect(tilePriority(5, 16, 16, camera)).toBe('high');
    expect(tilePriority(5, 18, 16, camera)).toBe('auto');
    expect(tilePriority(5, 25, 16, camera)).toBe('low');
    expect(tilePriority(8, 128, 128, camera)).toBe('low');
  });

  test('across the antimeridian, the short way', () => {
    expect(tilePriority(5, 0, 16, { center: [179, 0], zoom: 4 })).toBe('high');
  });
});
