/**
 * Decoding worker: fetches 3D Tiles content, decodes it with the engine's WebAssembly module (compiled once on the
 * main thread and shared) and its images with createImageBitmap, then transfers GPU-ready arrays back.
 */

import type { WorkerReply, WorkerRequest } from './protocol.ts';
import type { DecodedPrimitive } from './wasm/geo_render.js';
import init, { decodeTile } from './wasm/geo_render.js';

let ready: Promise<unknown> | undefined;
const controllers = new Map<number, AbortController>();

const reply = (message: WorkerReply, transfer: Transferable[] = []): void => postMessage(message, transfer);

addEventListener('message', (event: MessageEvent<WorkerRequest>) => {
  const message = event.data;
  switch (message.type) {
    case 'init':
      ready = init({ module_or_path: message.module });
      break;
    case 'abort':
      controllers.get(message.id)?.abort();
      break;
    case 'load':
      void load(message.id, message.url, message.transform, message.yUp);
      break;
  }
});

async function load(id: number, url: string, transform: Float64Array, yUp: boolean): Promise<void> {
  const controller = new AbortController();
  controllers.set(id, controller);
  try {
    await ready;
    const response = await fetch(url, { signal: controller.signal });
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    const tile = decodeTile(new Uint8Array(await response.arrayBuffer()), transform, yUp);
    const primitives: DecodedPrimitive[] = await Promise.all(
      tile.primitives.map(async ({ vertices, indices, image, mime }) => ({
        vertices,
        indices,
        bitmap: image
          ? await createImageBitmap(new Blob([image], { type: mime ?? '' }), { premultiplyAlpha: 'none' })
          : null,
      })),
    );
    const transfer = primitives.flatMap((p): Transferable[] => [
      p.vertices.buffer,
      p.indices.buffer,
      ...(p.bitmap ? [p.bitmap] : []),
    ]);
    reply({ type: 'tile', id, tile: { anchor: tile.anchor, primitives } }, transfer);
  } catch (error) {
    reply({ type: 'error', id, error: String(error), aborted: controller.signal.aborted });
  } finally {
    controllers.delete(id);
  }
}
