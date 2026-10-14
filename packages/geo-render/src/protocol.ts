/** Messages between the main thread and the decoding workers */
import type { DecodedTile } from './wasm/geo_render.js';

export type WorkerRequest =
  | { type: 'init'; module: WebAssembly.Module }
  | { type: 'load'; id: number; url: string; transform: Float64Array; yUp: boolean }
  | { type: 'abort'; id: number };

export type WorkerReply =
  | { type: 'tile'; id: number; tile: DecodedTile }
  | { type: 'error'; id: number; error: string; aborted: boolean };
