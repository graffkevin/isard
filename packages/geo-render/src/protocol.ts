/** Messages between the main thread and the decoding workers */
import type { DecoderUrls } from './decoders.ts';
import type { DecodedTile, TextureFormats } from './wasm/geo_render.js';

export type WorkerRequest =
  | { type: 'init'; module: WebAssembly.Module; decoders: DecoderUrls; formats: TextureFormats }
  | { type: 'load'; id: number; url: string; transform: Float64Array; yUp: boolean; reload: boolean }
  | { type: 'abort'; id: number };

export type WorkerReply =
  | { type: 'tile'; id: number; tile: DecodedTile }
  | { type: 'error'; id: number; error: string; aborted: boolean };
