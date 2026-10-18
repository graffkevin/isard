/**
 * Altitudes encoded in the colors of PNG tiles, decoded in the page for the relief: Terrarium (Mapzen, the Terrain
 * Tiles of AWS) and Terrain-RGB (Mapbox, MapTiler). The engine reads raw floats only; a loader gives it the altitudes
 * of each tile.
 */
import type { TerrainEncoding } from './types.ts';

const SIZE = 256;

/** Altitudes (metres, rows from the north) of RGBA pixels in an encoding; below `floor` raised to it */
export function decodeHeights(
  rgba: Uint8ClampedArray | Uint8Array,
  encoding: Exclude<TerrainEncoding, 'float32'>,
  floor = Number.NEGATIVE_INFINITY,
): Float32Array {
  const out = new Float32Array(rgba.length / 4);
  for (let i = 0; i < out.length; i += 1) {
    const [r, g, b] = [rgba[4 * i] ?? 0, rgba[4 * i + 1] ?? 0, rgba[4 * i + 2] ?? 0];
    const h = encoding === 'terrarium' ? r * 256 + g + b / 256 - 32768 : -10000 + (r * 65536 + g * 256 + b) * 0.1;
    out[i] = Math.max(h, floor);
  }
  return out;
}

/**
 * A loader of the engine's relief for PNG tiles of `url` ({z}, {x}, {y}) in an encoding: 256 × 256 altitudes, null
 * where the server has no tile; a network or server error rejects (retried). `floor`: the lowest altitude kept (the
 * sea floor of a worldwide model flattened at sea level)
 */
export function encodedTerrainLoader(
  url: string,
  encoding: Exclude<TerrainEncoding, 'float32'>,
  floor?: number,
): (z: number, x: number, y: number, signal: AbortSignal) => Promise<Float32Array | null> {
  let canvas: OffscreenCanvas | undefined;
  return async (z, x, y, signal) => {
    const response = await fetch(url.replace('{z}', String(z)).replace('{x}', String(x)).replace('{y}', String(y)), {
      signal,
    });
    if (response.status === 404 || response.status === 204 || response.status === 410) return null;
    if (!response.ok) throw new Error(`${response.url}: ${response.status}`);
    // The colors as stored: no color management, no premultiplied alpha
    const bitmap = await createImageBitmap(await response.blob(), {
      premultiplyAlpha: 'none',
      colorSpaceConversion: 'none',
    });
    canvas ??= new OffscreenCanvas(SIZE, SIZE);
    const g = canvas.getContext('2d', { willReadFrequently: true });
    if (!g) throw new Error('no 2D context');
    // Another size is resampled to the nearest pixel: an interpolated color is no altitude
    g.imageSmoothingEnabled = false;
    g.clearRect(0, 0, SIZE, SIZE);
    g.drawImage(bitmap, 0, 0, SIZE, SIZE);
    bitmap.close();
    return decodeHeights(g.getImageData(0, 0, SIZE, SIZE).data, encoding, floor);
  };
}
