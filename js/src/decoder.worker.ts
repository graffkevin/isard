/**
 * Decoding worker: fetches 3D Tiles content, decodes it with the engine's WebAssembly module (compiled once on the
 * main thread and shared) and its images with createImageBitmap, then transfers GPU-ready arrays back.
 */
import init, { decodeTile } from './wasm/geo_render.js';

interface Request {
  id: number;
  url: string;
  transform: Float64Array;
  yUp: boolean;
}

type Message = { init: WebAssembly.Module } | { abort: number } | Request;

let ready: Promise<unknown> | undefined;
const controllers = new Map<number, AbortController>();
const scope = self as unknown as {
  onmessage: ((e: MessageEvent<Message>) => void) | null;
  postMessage(message: unknown, transfer: Transferable[]): void;
};

scope.onmessage = (e) => {
  const message = e.data;
  if ('init' in message) {
    ready = init({ module_or_path: message.init });
  } else if ('abort' in message) {
    controllers.get(message.abort)?.abort();
  } else {
    void load(message);
  }
};

async function load({ id, url, transform, yUp }: Request): Promise<void> {
  const controller = new AbortController();
  controllers.set(id, controller);
  try {
    await ready;
    const response = await fetch(url, { signal: controller.signal });
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    const tile = decodeTile(new Uint8Array(await response.arrayBuffer()), transform, yUp) as {
      anchor: Float64Array;
      primitives: { vertices: Float32Array; indices: Uint32Array; image?: Uint8Array<ArrayBuffer>; mime?: string }[];
    };
    const primitives = await Promise.all(
      tile.primitives.map(async (p) => ({
        vertices: p.vertices,
        indices: p.indices,
        bitmap: p.image
          ? await createImageBitmap(new Blob([p.image], { type: p.mime }), { premultiplyAlpha: 'none' })
          : null,
      })),
    );
    const transfer: Transferable[] = primitives.flatMap((p) => [
      p.vertices.buffer,
      p.indices.buffer,
      ...(p.bitmap ? [p.bitmap] : []),
    ]);
    scope.postMessage({ id, tile: { anchor: tile.anchor, primitives } }, transfer);
  } catch (error) {
    scope.postMessage({ id, error: String(error), aborted: controller.signal.aborted }, []);
  } finally {
    controllers.delete(id);
  }
}
