/** The decoding worker's side of the protocol (src/protocol.ts): what it fetches, what it answers */
import { afterAll, beforeEach, describe, expect, test } from 'bun:test';
import type { WorkerReply, WorkerRequest } from '../src/protocol.ts';
import { pointsWasm, setGlobals, wasm } from './fakes.ts';

type FetchCall = { url: string; init: RequestInit & { priority?: string } };

let handler: ((event: { data: WorkerRequest }) => void) | undefined;
const replies: { message: WorkerReply; transfer: Transferable[] }[] = [];
const fetches: FetchCall[] = [];
/** What the next fetch does with its call */
let respond: (call: FetchCall) => Promise<Response> = async () => new Response(new Uint8Array([1, 2, 3]));

const restore = setGlobals({
  addEventListener: (type: string, h: typeof handler) => {
    if (type === 'message') handler = h;
  },
  postMessage: (message: WorkerReply, transfer: Transferable[] = []) => replies.push({ message, transfer }),
  fetch: (url: string, init: RequestInit) => {
    const call = { url, init };
    fetches.push(call);
    return respond(call);
  },
  createImageBitmap: async () => ({ close() {} }),
});
afterAll(restore);

// A computed specifier: the worker is type-checked with its own lib (tsconfig.worker.json), not Bun's
const workerModule = '../src/decoder.worker.ts';
await import(workerModule);

const send = (data: WorkerRequest) => handler?.({ data });
const transform = Float64Array.of(1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1);
const load = (id: number, extra: Partial<Extract<WorkerRequest, { type: 'load' }>> = {}) =>
  send({ type: 'load', id, url: `${id}.glb`, transform, yUp: true, reload: false, ...extra });
/** Waits for the worker to answer `count` messages */
const answered = async (count: number) => {
  for (let i = 0; i < 100 && replies.length < count; i += 1) await new Promise((r) => setTimeout(r, 0));
  return replies;
};

beforeEach(() => {
  replies.length = 0;
  fetches.length = 0;
  respond = async () => new Response(new Uint8Array([1, 2, 3]));
  wasm.decodeTile.mockClear();
  wasm.decodeTile.mockImplementation(() => ({ anchor: [0, 0, 0], primitives: [] }));
});

describe('decoding worker', () => {
  test('listens to messages; init starts the engine with the shared module', () => {
    expect(handler).toBeFunction();
    const module = {} as WebAssembly.Module;
    send({
      type: 'init',
      module,
      decoders: { draco: 'd.js', basis: 'b.js' } as never,
      formats: { astc: false, bptc: false, etc: false, s3tc: false },
      points: 'https://example.org/isard_points_bg.wasm',
    });
    expect(wasm.init).toHaveBeenLastCalledWith({ module_or_path: module });
    // The point cloud module waits for the first point cloud
    expect(pointsWasm.init).not.toHaveBeenCalled();
  });

  test('the priority goes to fetch only when given; a reload bypasses the HTTP cache', async () => {
    load(1, { priority: 'high' });
    load(2, { priority: 'low', reload: true });
    load(3);
    await answered(3);
    const [high, low, none] = fetches;
    expect(high?.url).toBe('1.glb');
    expect(high?.init.priority).toBe('high');
    expect(high?.init.cache).toBe('default');
    expect(high?.init.signal).toBeInstanceOf(AbortSignal);
    expect(low?.init.priority).toBe('low');
    expect(low?.init.cache).toBe('reload');
    expect(none && 'priority' in none.init).toBe(false);
  });

  test('a decoded tile is answered with its arrays transferred', async () => {
    const vertices = new Float32Array(6);
    const indices = new Uint32Array(3);
    wasm.decodeTile.mockImplementation(() => ({ anchor: [1, 2, 3], primitives: [{ vertices, indices }] }));
    load(4, { yUp: false });
    const [answer] = await answered(1);
    expect(wasm.decodeTile.mock.calls[0]?.[1]).toBe(transform);
    expect(wasm.decodeTile.mock.calls[0]?.[2]).toBe(false);
    expect(answer?.message).toEqual({
      type: 'tile',
      id: 4,
      tile: { anchor: [1, 2, 3], primitives: [{ vertices, indices, bitmap: null }] },
    } as unknown as WorkerReply);
    expect(answer?.transfer).toEqual([vertices.buffer, indices.buffer]);
  });

  test('an image becomes a bitmap, transferred too', async () => {
    const vertices = new Float32Array(3);
    const indices = new Uint32Array(3);
    const image = new Uint8Array([9]);
    wasm.decodeTile.mockImplementation(() => ({
      anchor: [0, 0, 0],
      primitives: [{ vertices, indices, image, mime: 'image/png' }],
    }));
    load(5);
    const [answer] = await answered(1);
    expect(answer?.message.type).toBe('tile');
    expect(answer?.transfer.length).toBe(3);
  });

  test('an HTTP error is answered as an error, not aborted', async () => {
    respond = async () => new Response('nope', { status: 404 });
    load(6);
    const [answer] = await answered(1);
    expect(answer?.message).toEqual({ type: 'error', id: 6, error: 'Error: HTTP 404', aborted: false });
    expect(wasm.decodeTile).not.toHaveBeenCalled();
  });

  test('an abort cancels the fetch of that tile only, answered as aborted', async () => {
    respond = ({ init }) =>
      new Promise((_, reject) =>
        init.signal?.addEventListener('abort', () => reject(new DOMException('aborted', 'AbortError'))),
      );
    load(7);
    load(8);
    await new Promise((r) => setTimeout(r, 0));
    send({ type: 'abort', id: 7 });
    const [answer] = await answered(1);
    expect(answer?.message).toMatchObject({ type: 'error', id: 7, aborted: true });
    expect(fetches[1]?.init.signal?.aborted).toBe(false);
    // An unknown id is harmless; the other tile is aborted in turn
    send({ type: 'abort', id: 99 });
    send({ type: 'abort', id: 8 });
    expect((await answered(2))[1]?.message).toMatchObject({ id: 8, aborted: true });
  });

  test('a decoding failure is answered as an error', async () => {
    wasm.decodeTile.mockImplementation(() => {
      throw new Error('bad glb');
    });
    load(9);
    const [answer] = await answered(1);
    expect(answer?.message).toEqual({ type: 'error', id: 9, error: 'Error: bad glb', aborted: false });
  });
});

describe('COPC chunks', () => {
  const laz = new Uint8Array([1, 2]);
  const layout = Float64Array.of(6, 30, 0.01, 0.01, 0.01, 0, 0, 0);
  const grid = new Float64Array(30);
  const points = (id: number, extra: Partial<Extract<WorkerRequest, { type: 'points' }>> = {}) =>
    send({ type: 'points', id, url: 'cloud.copc.laz', offset: 1000, size: 50, laz, layout, grid, count: 4, ...extra });
  const partial = (bytes = 50) => new Response(new Uint8Array(bytes), { status: 206 });

  beforeEach(() => {
    pointsWasm.init.mockClear();
    pointsWasm.decodeCopc.mockClear();
  });

  test('a chunk is fetched by its range, decoded by the point cloud module, its vertices transferred', async () => {
    respond = async () => partial();
    points(1, { priority: 'high' });
    const [answer] = await answered(1);
    expect(fetches[0]?.url).toBe('cloud.copc.laz');
    expect(fetches[0]?.init.headers).toEqual({ Range: 'bytes=1000-1049' });
    expect(fetches[0]?.init.priority).toBe('high');
    expect(pointsWasm.init).toHaveBeenCalledWith({ module_or_path: 'https://example.org/isard_points_bg.wasm' });
    const [chunk, givenLaz, givenLayout, givenGrid, count] = pointsWasm.decodeCopc.mock.calls[0] ?? [];
    expect([chunk?.length, givenLaz, givenLayout, givenGrid, count]).toEqual([50, laz, layout, grid, 4]);
    expect(answer?.message.type).toBe('points');
    const vertices = (answer?.message as { vertices: Uint8Array } | undefined)?.vertices ?? new Uint8Array();
    expect(vertices.length).toBe(80);
    expect(answer?.transfer).toEqual([vertices.buffer]);
    // Loaded once for every chunk
    points(2);
    await answered(2);
    expect(pointsWasm.init).toHaveBeenCalledTimes(1);
  });

  test('a server turning requests away (429) is asked again after a pause', async () => {
    let calls = 0;
    respond = async () => {
      calls += 1;
      return calls === 1 ? new Response('', { status: 429, headers: { 'Retry-After': '0.01' } }) : partial();
    };
    points(3);
    const [answer] = await answered(1);
    expect(calls).toBe(2);
    expect(answer?.message.type).toBe('points');
  });

  test('a server ignoring the range is an error, its body not read; a decoding failure too', async () => {
    respond = async () => new Response(new Uint8Array(10), { status: 200, headers: { 'Content-Length': '99999' } });
    points(4);
    const [answer] = await answered(1);
    expect(answer?.message).toMatchObject({ type: 'error', id: 4, aborted: false });
    expect(String((answer?.message as { error: string } | undefined)?.error)).toContain('range');
    expect(pointsWasm.decodeCopc).not.toHaveBeenCalled();
    respond = async () => partial();
    pointsWasm.decodeCopc.mockImplementationOnce(() => {
      throw new Error('LAZ chunk: failed to fill whole buffer');
    });
    points(5);
    expect((await answered(2))[1]?.message).toMatchObject({ type: 'error', id: 5 });
  });

  test('an aborted chunk is answered as aborted', async () => {
    respond = ({ init }) =>
      new Promise((_, reject) =>
        init.signal?.addEventListener('abort', () => reject(new DOMException('aborted', 'AbortError'))),
      );
    points(6);
    await new Promise((r) => setTimeout(r, 0));
    send({ type: 'abort', id: 6 });
    expect((await answered(1))[0]?.message).toMatchObject({ type: 'error', id: 6, aborted: true });
  });
});
