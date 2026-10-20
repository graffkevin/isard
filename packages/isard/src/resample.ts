/**
 * Raster reprojection, the drawing half: a Web Mercator tile drawn from the source images it spans (tiles of another
 * grid, or one image of a WMS), in a worker of the map (OffscreenCanvas) or in the page when the map has no workers.
 * The page plans each tile (`grid.ts`): which images, where they go in a mosaic, and a mesh of control points giving,
 * for the corners of 16 × 16 cells of the tile, the mosaic pixel under them. Here the images are fetched (kept for the
 * neighbouring tiles), drawn into the mosaic, and every pixel of the tile sampled from it, its source position
 * interpolated within its cell (bilinear), its color between the four nearest source pixels (bilinear, alpha
 * premultiplied). This code knows no projection.
 */
import { Lru } from './lru.ts';
import { withHeaders } from './requests.ts';

/** Source images kept decoded (by the page, or shared by the workers of a pool): the neighbours of a tile share them */
export const SOURCE_CACHE = 96;

/** A source image and the rectangle of the mosaic it covers (pixels) */
export interface Placed {
  url: string;
  x: number;
  y: number;
  w: number;
  h: number;
}

/** How to draw a Mercator tile of `size` pixels: the mosaic of the source images, and the mesh of `n` × `n` cells
 * ((n + 1)² points, rows from the north, x then y): the mosaic position of each corner, NaN where the source CRS has
 * none */
export interface Job {
  size: number;
  width: number;
  height: number;
  sources: Placed[];
  n: number;
  mesh: Float32Array;
}

/**
 * Samples the pixels of a `size` × `size` tile (RGBA, straight alpha) from a mosaic of `width` × `height` (RGBA,
 * straight alpha) through the mesh of a job. Pixels whose position falls outside the mosaic, or on a NaN, stay
 * transparent; the mosaic's edge fades over half a pixel
 */
export function resample(
  source: Uint8ClampedArray,
  width: number,
  height: number,
  mesh: Float32Array,
  n: number,
  size: number,
): Uint8ClampedArray<ArrayBuffer> {
  const out = new Uint8ClampedArray(size * size * 4);
  const cell = size / n;
  const stride = n + 1;
  for (let row = 0; row < size; row += 1) {
    const fy = (row + 0.5) / cell;
    const j = Math.min(n - 1, Math.floor(fy));
    const ty = fy - j;
    for (let col = 0; col < size; col += 1) {
      const fx = (col + 0.5) / cell;
      const i = Math.min(n - 1, Math.floor(fx));
      const tx = fx - i;
      const a = 2 * (j * stride + i);
      const b = a + 2 * stride;
      // The corners of the cell, then the position in it
      const w00 = (1 - tx) * (1 - ty);
      const w10 = tx * (1 - ty);
      const w01 = (1 - tx) * ty;
      const w11 = tx * ty;
      const u =
        w00 * (mesh[a] as number) +
        w10 * (mesh[a + 2] as number) +
        w01 * (mesh[b] as number) +
        w11 * (mesh[b + 2] as number);
      const v =
        w00 * (mesh[a + 1] as number) +
        w10 * (mesh[a + 3] as number) +
        w01 * (mesh[b + 1] as number) +
        w11 * (mesh[b + 3] as number);
      // Pixel centers at half pixels: the four around (u, v)
      const sx = u - 0.5;
      const sy = v - 0.5;
      if (!(sx > -1 && sy > -1 && sx < width && sy < height)) continue;
      const x0 = Math.floor(sx);
      const y0 = Math.floor(sy);
      const kx = sx - x0;
      const ky = sy - y0;
      let r = 0;
      let g = 0;
      let bl = 0;
      let alpha = 0;
      for (let k = 0; k < 4; k += 1) {
        const x = x0 + (k & 1);
        const y = y0 + (k >> 1);
        if (x < 0 || y < 0 || x >= width || y >= height) continue;
        const p = 4 * (y * width + x);
        const wa = (k & 1 ? kx : 1 - kx) * (k >> 1 ? ky : 1 - ky) * (source[p + 3] as number);
        r += wa * (source[p] as number);
        g += wa * (source[p + 1] as number);
        bl += wa * (source[p + 2] as number);
        alpha += wa;
      }
      if (alpha <= 0) continue;
      const q = 4 * (row * size + col);
      out[q] = r / alpha;
      out[q + 1] = g / alpha;
      out[q + 2] = bl / alpha;
      out[q + 3] = alpha;
    }
  }
  return out;
}

/** Draws a planned tile: its image, null where the source has nothing; rejects on errors */
export type DrawJob = (job: Job, signal: AbortSignal) => Promise<ImageBitmap | null>;

/**
 * Draws planned tiles, keeping `cached` source images decoded. A missing source image (404, 204, 410) leaves its part
 * empty; a tile with no source at all is missing; other errors reject (retried by the engine). In the page, or in a
 * worker of the map (`close` frees the images kept)
 */
export function resampler(cached = SOURCE_CACHE): { draw: DrawJob; close: () => void; cache: { capacity: number } } {
  type Image = Promise<ImageBitmap | null>;
  /** Tiles being drawn from each source image: an evicted one is closed once none is */
  const users = new Map<Image, number>();
  const evicted = new Set<Image>();
  const close = (image: Image) => {
    evicted.delete(image);
    image.then(
      (bitmap) => bitmap?.close(),
      () => undefined,
    );
  };
  /** Freed now, or once the tiles drawing from it are done */
  const drop = (image: Image) => {
    if (users.has(image)) evicted.add(image);
    else close(image);
  };
  // Source images by URL: a request is shared by the tiles that need it, its signal is not theirs
  const cache = new Lru<string, Image>(cached, drop);
  const release = (image: Image) => {
    const count = (users.get(image) ?? 1) - 1;
    if (count > 0) users.set(image, count);
    else {
      users.delete(image);
      if (evicted.has(image)) close(image);
    }
  };
  const fetchImage = (url: string): Image => {
    let image = cache.get(url);
    if (!image) {
      image = (async () => {
        const response = await fetch(url, withHeaders(url));
        if (response.status === 404 || response.status === 204 || response.status === 410) return null;
        if (!response.ok) throw new Error(`isard: ${response.status} for a raster to reproject`);
        return createImageBitmap(await response.blob());
      })();
      // A failure is not kept: the engine's retry fetches again
      image.catch(() => cache.delete(url));
      cache.set(url, image);
    }
    users.set(image, (users.get(image) ?? 0) + 1);
    return image;
  };
  const draw: DrawJob = async (job, signal) => {
    const pending = job.sources.map((s) => [s, fetchImage(s.url)] as const);
    try {
      const images = await Promise.all(pending.map(async ([s, image]) => [s, await image] as const));
      signal.throwIfAborted();
      if (!images.some(([, image]) => image)) return null;
      const canvas = new OffscreenCanvas(job.width, job.height);
      const context = canvas.getContext('2d', { willReadFrequently: true });
      if (!context) throw new Error('isard: no 2D canvas to reproject the tiles');
      for (const [s, image] of images) if (image) context.drawImage(image, s.x, s.y, s.w, s.h);
      const mosaic = context.getImageData(0, 0, job.width, job.height).data;
      const pixels = resample(mosaic, job.width, job.height, job.mesh, job.n, job.size);
      return createImageBitmap(new ImageData(pixels, job.size, job.size), { premultiplyAlpha: 'none' });
    } finally {
      for (const [, image] of pending) release(image);
    }
  };
  return {
    draw,
    cache,
    close: () => {
      for (const image of cache.values()) drop(image);
      cache.clear();
    },
  };
}
