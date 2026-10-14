/**
 * Decoding worker: fetches 3D Tiles content, decodes it with the engine's WebAssembly module (compiled once on the
 * main thread and shared), its Draco geometry and KTX2 textures with the optional decoders (loaded on first use), its
 * other images with createImageBitmap, then transfers GPU-ready arrays back.
 */
import { DEFAULT_DECODERS, type DecoderUrls, decodeDraco, transcodeKtx2 } from './decoders.ts';
import type { WorkerReply, WorkerRequest } from './protocol.ts';
import type { DecodedPrimitive, TextureFormats } from './wasm/geo_render.js';
import init, { decodeTile, dracoRequests } from './wasm/geo_render.js';

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
      void load(message.id, message.url, message.transform, message.yUp, message.reload);
      break;
  }
});

async function load(id: number, url: string, transform: Float64Array, yUp: boolean, reload: boolean): Promise<void> {
  const controller = new AbortController();
  controllers.set(id, controller);
  try {
    await ready;
    // A retried tile bypasses the HTTP cache: it may hold the error
    const response = await fetch(url, { signal: controller.signal, cache: reload ? 'reload' : 'default' });
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    const bytes = new Uint8Array(await response.arrayBuffer());
    // Draco geometry first (its module loads on first use), then the tile itself
    const requests = dracoRequests(bytes);
    const draco = requests.length > 0 ? await decodeDraco(decoders, requests) : undefined;
    const tile = decodeTile(bytes, transform, yUp, draco);
    const primitives = await Promise.all(
      tile.primitives.map(async ({ vertices, indices, image, mime }): Promise<DecodedPrimitive> => {
        if (!image) return { vertices, indices, bitmap: null };
        if (mime === 'image/ktx2') {
          return { vertices, indices, bitmap: null, texture: await transcodeKtx2(decoders, formats, image) };
        }
        const blob = new Blob([image], { type: mime ?? '' });
        return { vertices, indices, bitmap: await createImageBitmap(blob, { premultiplyAlpha: 'none' }) };
      }),
    );
    const transfer = primitives.flatMap((p): Transferable[] => [
      p.vertices.buffer,
      p.indices.buffer,
      ...(p.bitmap ? [p.bitmap] : []),
      ...(p.texture?.levels.map((level) => level.data.buffer) ?? []),
    ]);
    reply({ type: 'tile', id, tile: { anchor: tile.anchor, primitives } }, transfer);
  } catch (error) {
    reply({ type: 'error', id, error: String(error), aborted: controller.signal.aborted });
  } finally {
    controllers.delete(id);
  }
}
