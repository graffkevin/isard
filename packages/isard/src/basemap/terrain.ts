/**
 * Altitudes encoded in the colors of PNG tiles, for the relief: Terrarium (Mapzen, the Terrain Tiles of AWS) and
 * Terrain-RGB (Mapbox, MapTiler). The engine reads raw floats only; a loader gives it the altitudes of each tile,
 * decoded in a worker of the map's pool (OffscreenCanvas), or in the page when the map has no workers.
 */
import { withHeaders } from '../engine/requests.ts';

/** The encodings of altitudes in the colors of PNG tiles */
export type PngEncoding = 'terrarium' | 'mapbox';

const SIZE = 256;

/** Altitudes (metres, rows from the north) of RGBA pixels in an encoding; below `floor` raised to it */
export function decodeHeights(
  rgba: Uint8ClampedArray | Uint8Array,
  encoding: PngEncoding,
  floor = Number.NEGATIVE_INFINITY,
): Float32Array {
  const n = rgba.length >> 2;
  const out = new Float32Array(n);
  // A loop per encoding, without allocations: 65 536 pixels a tile
  if (encoding === 'terrarium') {
    for (let i = 0, j = 0; i < n; i += 1, j += 4) {
      const h = (rgba[j] as number) * 256 + (rgba[j + 1] as number) + (rgba[j + 2] as number) / 256 - 32768;
      out[i] = h > floor ? h : floor;
    }
  } else {
    for (let i = 0, j = 0; i < n; i += 1, j += 4) {
      const h = -10000 + ((rgba[j] as number) * 65536 + (rgba[j + 1] as number) * 256 + (rgba[j + 2] as number)) * 0.1;
      out[i] = h > floor ? h : floor;
    }
  }
  return out;
}

/** Where the tiles are drawn and read back, made on first use and kept (each tile drawn and read at once) */
export interface Surface {
  canvas?: OffscreenCanvas;
}

/**
 * The altitudes of the PNG tile at `url`: 256 × 256, null where the server has no tile; a network or server error
 * rejects (the engine retries). Where it runs: in a worker of the map, or in the page without workers
 */
export async function fetchHeights(
  url: string,
  encoding: PngEncoding,
  floor: number | undefined,
  signal: AbortSignal,
  surface: Surface,
): Promise<Float32Array | null> {
  const response = await fetch(url, withHeaders(url, { signal }));
  if (response.status === 404 || response.status === 204 || response.status === 410) return null;
  if (!response.ok) throw new Error(`${response.url}: ${response.status}`);
  // The colors as stored: no color management, no premultiplied alpha
  const bitmap = await createImageBitmap(await response.blob(), {
    premultiplyAlpha: 'none',
    colorSpaceConversion: 'none',
  });
  try {
    surface.canvas ??= new OffscreenCanvas(SIZE, SIZE);
    const g = surface.canvas.getContext('2d', { willReadFrequently: true });
    if (!g) throw new Error('no 2D context');
    // Another size is resampled to the nearest pixel: an interpolated color is no altitude
    g.imageSmoothingEnabled = false;
    g.clearRect(0, 0, SIZE, SIZE);
    g.drawImage(bitmap, 0, 0, SIZE, SIZE);
    return decodeHeights(g.getImageData(0, 0, SIZE, SIZE).data, encoding, floor);
  } finally {
    bitmap.close();
  }
}

/** `fetchHeights` run in a worker for tile `z`/`x`/`y`; undefined when the map has no workers (run in the page) */
export type RemoteHeights = (
  url: string,
  encoding: PngEncoding,
  floor: number | undefined,
  tile: [z: number, x: number, y: number],
  signal: AbortSignal,
) => Promise<Float32Array | null> | undefined;

/**
 * A loader of the engine's relief for PNG tiles of `url` ({z}, {x}, {y}) in an encoding: 256 × 256 altitudes, null
 * where the server has no tile; a network or server error rejects (retried). `floor`: the lowest altitude kept (the
 * sea floor of a worldwide model flattened at sea level). `remote`: decodes in a worker when it can
 */
export function encodedTerrainLoader(
  url: string,
  encoding: PngEncoding,
  floor?: number,
  remote?: RemoteHeights,
): (z: number, x: number, y: number, signal: AbortSignal) => Promise<Float32Array | null> {
  const surface: Surface = {};
  return (z, x, y, signal) => {
    const tile = url.replaceAll('{z}', String(z)).replaceAll('{x}', String(x)).replaceAll('{y}', String(y));
    return remote?.(tile, encoding, floor, [z, x, y], signal) ?? fetchHeights(tile, encoding, floor, signal, surface);
  };
}
