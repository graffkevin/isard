/**
 * Decoding worker: fetches 3D Tiles content, decodes it with the engine's WebAssembly module (compiled once on the
 * main thread and shared), its Draco geometry and KTX2 textures with the optional decoders (loaded on first use), its
 * other images with createImageBitmap, then transfers GPU-ready arrays back.
 */
import { DEFAULT_DECODERS, type DecoderUrls, decodeDraco, transcodeKtx2 } from './decoders.ts';
import type { WorkerReply, WorkerRequest } from './protocol.ts';
import type { DecodedPrimitive, TextureFormats } from './wasm/isard.js';
import init, { decodeTile, dracoRequests } from './wasm/isard.js';

let ready: Promise<unknown> | undefined;
let decoders: DecoderUrls = DEFAULT_DECODERS;
let formats: TextureFormats = { astc: false, bptc: false, etc: false, s3tc: false };
const controllers = new Map<number, AbortController>();

const reply = (message: WorkerReply, transfer: Transferable[] = []): void => postMessage(message, transfer);

addEventListener('message', (event: MessageEvent<WorkerRequest>) => {
  const message = event.data;
  switch (message.type) {
    case 'init':
      ready = init({ module_or_path: message.module });
      decoders = message.decoders;
      formats = message.formats;
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
    reply({ type: 'tile', id, tile: { anchor: tile.anchor, primitives } }, [...transfer]);
  } catch (error) {
    reply({ type: 'error', id, error: String(error), aborted: controller.signal.aborted });
  } finally {
    controllers.delete(id);
  }
}
