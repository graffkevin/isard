/** Messages between the main thread and the decoding workers */

import type { DecodedTile, TextureFormats } from '../wasm/isard.js';
import type { DecoderUrls } from './decoders.ts';
import type { HeaderRules } from './requests.ts';

export type WorkerRequest =
  /** The page's request headers (`setRequestHeaders`) */
  | { type: 'headers'; rules: HeaderRules }
  | {
      type: 'init';
      module: WebAssembly.Module;
      decoders: DecoderUrls;
      formats: TextureFormats;
      /** URL of the point cloud module (`isard_points_bg.wasm`), loaded with the first COPC chunk */
      points: string;
    }
  | {
      type: 'load';
      id: number;
      url: string;
      transform: Float64Array;
      yUp: boolean;
      reload: boolean;
      priority?: 'high' | 'low';
    }
  | {
      type: 'points';
      id: number;
      url: string;
      offset: number;
      size: number;
      laz: Uint8Array;
      layout: Float64Array;
      grid: Float64Array;
      count: number;
      priority?: 'high' | 'low';
    }
  | { type: 'abort'; id: number };

export type WorkerReply =
  | { type: 'tile'; id: number; tile: DecodedTile }
  | { type: 'points'; id: number; vertices: Uint8Array<ArrayBuffer> }
  | { type: 'error'; id: number; error: string; aborted: boolean };
