/**
 * Decoding worker: fetches 3D Tiles content, decodes it with the engine's WebAssembly module (compiled once on the
 * main thread and shared), its Draco geometry and KTX2 textures with the optional decoders (loaded on first use), its
 * other images with createImageBitmap, then transfers GPU-ready arrays back. The chunks of COPC point clouds are
 * fetched by range requests and decoded by the point cloud module (the LAZ decoder), loaded with the first one.
 */
import { DEFAULT_DECODERS, type DecoderUrls, decodeDraco, transcodeKtx2 } from './decoders.ts';
import type { WorkerReply, WorkerRequest } from './protocol.ts';
import type { DecodedPrimitive, TextureFormats } from './wasm/isard.js';
import init, { decodeTile, dracoRequests } from './wasm/isard.js';
import initPoints, { decodeCopc } from './wasm/isard_points.js';

let ready: Promise<unknown> | undefined;
let decoders: DecoderUrls = DEFAULT_DECODERS;
let formats: TextureFormats = { astc: false, bptc: false, etc: false, s3tc: false };
/** The point cloud module: its URL, and its instance once loaded */
let pointsUrl = '';
let points: Promise<unknown> | undefined;
const controllers = new Map<number, AbortController>();

const reply = (message: WorkerReply, transfer: Transferable[] = []): void => postMessage(message, transfer);

addEventListener('message', (event: MessageEvent<WorkerRequest>) => {
  const message = event.data;
  switch (message.type) {
    case 'init':
      ready = init({ module_or_path: message.module });
      decoders = message.decoders;
      formats = message.formats;
      pointsUrl = message.points;
      break;
    case 'points':
      void loadPoints(message);
      break;
    case 'abort':
      controllers.get(message.id)?.abort();
      break;
    case 'load':
      void load(message.id, message.url, message.transform, message.yUp, message.reload, message.priority);
      break;
  }
});

async function load(
  id: number,
  url: string,
  transform: Float64Array,
  yUp: boolean,
  reload: boolean,
  priority: 'high' | 'low' | undefined,
): Promise<void> {
  const controller = new AbortController();
  controllers.set(id, controller);
  try {
    await ready;
    // A retried tile bypasses the HTTP cache: it may hold the error
    // Fetch priority (tiles in view first): not yet in TypeScript's lib
    const init: RequestInit & { priority?: 'high' | 'low' } = {
      signal: controller.signal,
      cache: reload ? 'reload' : 'default',
      ...(priority ? { priority } : {}),
    };
    const response = await fetch(url, init);
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    const bytes = new Uint8Array(await response.arrayBuffer());
    // Draco geometry first (its module loads on first use), then the tile itself
    const requests = dracoRequests(bytes);
    const draco = requests.length > 0 ? await decodeDraco(decoders, requests) : undefined;
    const tile = decodeTile(bytes, transform, yUp, draco);
    // An image shared by primitives (instanced meshes) is decoded once: the engine then shares its texture
    type Image = NonNullable<(typeof tile.primitives)[number]['image']>;
    const decoded = new Map<Image, Promise<Pick<DecodedPrimitive, 'bitmap' | 'texture'>>>();
    const decode = (image: Image, mime: string | undefined) => {
      let pending = decoded.get(image);
      if (!pending) {
        pending =
          mime === 'image/ktx2'
            ? transcodeKtx2(decoders, formats, image).then((texture) => ({ bitmap: null, texture }))
            : createImageBitmap(new Blob([image], { type: mime ?? '' }), { premultiplyAlpha: 'none' }).then(
                (bitmap) => ({ bitmap }),
              );
        decoded.set(image, pending);
      }
      return pending;
    };
    const vertices = tile.points ?? [];
    const primitives = await Promise.all(
      tile.primitives.map(
        async ({ vertices, indices, image, mime, color }): Promise<DecodedPrimitive> => ({
          vertices,
          indices,
          ...(color ? { color } : {}),
          ...(image ? await decode(image, mime) : { bitmap: null }),
        }),
      ),
    );
    // Each object once: a buffer or bitmap listed twice makes the transfer throw
    const transfer = new Set<Transferable>();
    for (const p of primitives) {
      transfer.add(p.vertices.buffer);
      transfer.add(p.indices.buffer);
      if (p.color) transfer.add(p.color.buffer);
      if (p.bitmap) transfer.add(p.bitmap);
      for (const level of p.texture?.levels ?? []) transfer.add(level.data.buffer);
    }
    for (const v of vertices) transfer.add(v.buffer);
    const decodedTile = { anchor: tile.anchor, primitives, ...(vertices.length > 0 ? { points: vertices } : {}) };
    reply({ type: 'tile', id, tile: decodedTile }, [...transfer]);
  } catch (error) {
    reply({ type: 'error', id, error: String(error), aborted: controller.signal.aborted });
  } finally {
    controllers.delete(id);
  }
}

/** Attempts at a chunk the server turned away for too many requests (HTTP 429), and the first pause (ms, doubled) */
const BUSY_ATTEMPTS = 4;
const BUSY_PAUSE_MS = 1000;

const sleep = (ms: number, signal: AbortSignal) =>
  new Promise<void>((resolve, reject) => {
    const timer = setTimeout(resolve, ms);
    signal.addEventListener('abort', () => {
      clearTimeout(timer);
      reject(new DOMException('aborted', 'AbortError'));
    });
  });

/** A range of a file; a server that ignores the range (200 for more than asked) is an error, its body not read */
async function fetchRange(
  url: string,
  offset: number,
  size: number,
  signal: AbortSignal,
  priority: 'high' | 'low' | undefined,
): Promise<Uint8Array> {
  for (let attempt = 1; ; attempt += 1) {
    const init: RequestInit & { priority?: 'high' | 'low' } = {
      signal,
      headers: { Range: `bytes=${offset}-${offset + Math.max(size, 1) - 1}` },
      ...(priority ? { priority } : {}),
    };
    const response = await fetch(url, init);
    if (response.status === 429 && attempt < BUSY_ATTEMPTS) {
      const after = Number(response.headers.get('Retry-After'));
      await sleep(after > 0 ? after * 1000 : BUSY_PAUSE_MS * 2 ** (attempt - 1), signal);
      continue;
    }
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    if (response.status !== 206) {
      const length = Number(response.headers.get('Content-Length'));
      if (!(length <= size)) {
        await response.body?.cancel();
        throw new Error('the server ignores range requests (HTTP 206 expected)');
      }
      return new Uint8Array(await response.arrayBuffer()).subarray(offset, offset + size);
    }
    return new Uint8Array(await response.arrayBuffer());
  }
}

/** The points of a COPC node: its chunk fetched, decompressed and positioned by the point cloud module */
async function loadPoints(message: Extract<WorkerRequest, { type: 'points' }>): Promise<void> {
  const { id, url, offset, size, laz, layout, grid, count, priority } = message;
  const controller = new AbortController();
  controllers.set(id, controller);
  try {
    points ??= initPoints({ module_or_path: pointsUrl }).catch((error: unknown) => {
      // A later chunk tries again
      points = undefined;
      throw error;
    });
    const [bytes] = await Promise.all([fetchRange(url, offset, size, controller.signal, priority), points]);
    const vertices = decodeCopc(bytes, laz, layout, grid, count) as Uint8Array<ArrayBuffer>;
    reply({ type: 'points', id, vertices }, [vertices.buffer]);
  } catch (error) {
    reply({ type: 'error', id, error: String(error), aborted: controller.signal.aborted });
  } finally {
    controllers.delete(id);
  }
}
