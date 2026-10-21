/**
 * Rasters in another tile grid, the planning half: a WMTS-like tile matrix set in any CRS (Lambert-93, UTM, longitudes
 * and latitudes…), or a WMS asked for a box in that CRS, reprojected into the Web Mercator tiles the engine draws. For
 * each Mercator tile the page projects a mesh of 17 × 17 points of the tile into the source CRS (the only projection
 * work: 289 points a tile, whatever its size), picks the source level of the same pixel size, and lists the source
 * images the tile spans; the drawing (`resample.ts`) is done by a worker of the map, or in the page without workers.
 */
import { type Crs, type CrsLike, getCrs } from '../data/crs.ts';
import { latitude, longitude, mercatorX, mercatorY } from '../util/geo.ts';
import { type Job, type Placed, resampler } from './resample.ts';

/** One level of a tile grid, as a `TileMatrix` of WMTS */
export interface TileMatrix {
  /** What replaces {z} in the URL (the `TILEMATRIX` of a WMTS; default the index of the matrix in the grid) */
  id?: string | number;
  /** Size of a pixel, CRS units (WMTS: its `ScaleDenominator` × 0.00028 m, in CRS units) */
  resolution: number;
  /** Top-left corner of the tile (0, 0), CRS units (x east, y north) */
  origin: [x: number, y: number];
  /** Size of a tile, pixels (default 256) */
  tileWidth?: number;
  tileHeight?: number;
  /** Tiles across and down (default unbounded) */
  matrixWidth?: number;
  matrixHeight?: number;
  /** The tiles the source has (a WMTS's `TileMatrixLimits`), inclusive: none is asked for outside */
  limits?: [minCol: number, minRow: number, maxCol: number, maxRow: number];
}

/**
 * A raster's tile grid in a CRS of its own, reprojected into Web Mercator by the map: `matrices` for tiles ({z}, {x},
 * {y} in the URL: matrix, column, row, rows from the north), none for a WMS whose URL has `{bbox}` (the box of each
 * Mercator tile in that CRS: minimum x, minimum y, maximum x, maximum y, asked for at the tile's size)
 */
export interface TileGrid {
  /** `'EPSG:2154'`, another built-in or registered name, or an object with `forward` and `inverse` (proj4's) */
  crs: CrsLike;
  /** The levels of the grid, in any order */
  matrices?: TileMatrix[];
  /** Longitudes and latitudes the source covers (west, south, east, north): nothing is asked for outside */
  bounds?: [west: number, south: number, east: number, north: number];
  /** Identifier of the tile matrix set (a WMTS's `TILEMATRIXSET`), for the helpers that write the URL */
  name?: string;
}

/** Cells of the mesh along each side of a tile */
const N = 16;
/** Source levels finer at most than the pixel size across, where the grid stretches one axis (towards the poles in
 * longitudes and latitudes) */
const MAX_BOOST = 3;
/** Pixels of a mosaic at most: coarser levels beyond, nothing if even the coarsest is larger */
const MAX_MOSAIC = 2048 * 2048;

interface Matrix extends Required<Omit<TileMatrix, 'limits' | 'id'>> {
  id: string | number;
  limits: [number, number, number, number] | undefined;
}

interface Resolved {
  crs: Crs;
  /** Coarsest first */
  matrices: Matrix[];
  bounds: [number, number, number, number] | undefined;
  /** The bounds in the CRS: minimum x, minimum y, maximum x, maximum y */
  extent: [number, number, number, number] | undefined;
}

/** A box of the CRS around points of it (x, y, x, y…), NaN ignored; undefined without any */
function box(points: ArrayLike<number>): [number, number, number, number] | undefined {
  let [x0, y0, x1, y1] = [Infinity, Infinity, -Infinity, -Infinity];
  for (let k = 0; k + 1 < points.length; k += 2) {
    const [x, y] = [points[k] as number, points[k + 1] as number];
    if (!Number.isFinite(x) || !Number.isFinite(y)) continue;
    x0 = Math.min(x0, x);
    y0 = Math.min(y0, y);
    x1 = Math.max(x1, x);
    y1 = Math.max(y1, y);
  }
  return x0 <= x1 ? [x0, y0, x1, y1] : undefined;
}

/** A tile grid ready to plan tiles: its CRS found, its matrices completed, its bounds projected */
export function resolveGrid(grid: TileGrid): Resolved {
  const crs = getCrs(grid.crs);
  const matrices = (grid.matrices ?? [])
    .map(
      (m, i): Matrix => ({
        id: m.id ?? i,
        resolution: m.resolution,
        origin: m.origin,
        tileWidth: m.tileWidth ?? 256,
        tileHeight: m.tileHeight ?? 256,
        matrixWidth: m.matrixWidth ?? Infinity,
        matrixHeight: m.matrixHeight ?? Infinity,
        limits: m.limits,
      }),
    )
    .sort((a, b) => b.resolution - a.resolution);
  let extent: Resolved['extent'];
  if (grid.bounds) {
    // Its outline and inside, projected: the box of the CRS around them, a little larger (curved edges)
    const [w, s, e, n] = grid.bounds;
    const points: number[] = [];
    for (let i = 0; i <= 8; i += 1) {
      for (let j = 0; j <= 8; j += 1) points.push(...crs.forward([w + ((e - w) * i) / 8, s + ((n - s) * j) / 8]));
    }
    const b = box(points);
    if (b) {
      const margin = 0.01 * Math.max(b[2] - b[0], b[3] - b[1]);
      extent = [b[0] - margin, b[1] - margin, b[2] + margin, b[3] + margin];
    }
  }
  return { crs, matrices, bounds: grid.bounds, extent };
}

/** The longitudes and latitudes of tiles in longitudes and latitudes, two 256 px tiles side by side at level 0, rows
 * from the north (NASA's Trek, WorldCRS84Quad), levels `minLevel` to `maxLevel` ({z} is the level) */
export function equirectangularGrid(minLevel: number, maxLevel: number): TileGrid {
  const matrices: TileMatrix[] = [];
  for (let level = minLevel; level <= maxLevel; level += 1) {
    const n = 2 ** level;
    matrices.push({ id: level, resolution: 180 / 256 / n, origin: [-180, 90], matrixWidth: 2 * n, matrixHeight: n });
  }
  return { crs: 'EPSG:4326', matrices };
}

/** A URL of the source: {z}, {x}, {y}, {bbox}, {width} and {height} replaced */
function sourceUrl(template: string, z: string | number, x: number, y: number, bbox: number[], w: number, h: number) {
  return template
    .replaceAll('{z}', String(z))
    .replaceAll('{x}', String(x))
    .replaceAll('{y}', String(y))
    .replaceAll('{bbox}', bbox.join(','))
    .replaceAll('{width}', String(w))
    .replaceAll('{height}', String(h));
}

/** CRS units per pixel along the mesh's x and y around its middle (NaN where the CRS has nothing there) */
function pixelSize(points: Float64Array, size: number): [number, number] {
  const c = N / 2;
  const at = (i: number, j: number) => 2 * (j * (N + 1) + i);
  const d = (p: number, q: number) =>
    Math.hypot((points[q] as number) - (points[p] as number), (points[q + 1] as number) - (points[p + 1] as number));
  const step = (2 * size) / N;
  return [d(at(c - 1, c), at(c + 1, c)) / step, d(at(c, c - 1), at(c, c + 1)) / step];
}

/**
 * The box (CRS) of the points a tile samples: the centers of its edge pixels, interpolated in the mesh as `resample`
 * does, and the inner points of the mesh (should the projection bend); within `clip`. Undefined if none is
 */
function sampled(points: Float64Array, size: number, clip: number[]): [number, number, number, number] | undefined {
  const at = (i: number, j: number, k: number) => points[2 * (j * (N + 1) + i) + k] as number;
  const centers: number[] = [];
  for (let j = 1; j < N; j += 1) for (let i = 1; i < N; i += 1) centers.push(at(i, j, 0), at(i, j, 1));
  const cell = size / N;
  const add = (px: number, py: number) => {
    const [fx, fy] = [px / cell, py / cell];
    const [i, j] = [Math.min(N - 1, Math.floor(fx)), Math.min(N - 1, Math.floor(fy))];
    const [tx, ty] = [fx - i, fy - j];
    for (const k of [0, 1]) {
      const top = at(i, j, k) + (at(i + 1, j, k) - at(i, j, k)) * tx;
      const bottom = at(i, j + 1, k) + (at(i + 1, j + 1, k) - at(i, j + 1, k)) * tx;
      centers.push(top + (bottom - top) * ty);
    }
  };
  for (let p = 0.5; p < size; p += 1) {
    add(p, 0.5);
    add(p, size - 0.5);
    add(0.5, p);
    add(size - 0.5, p);
  }
  const b = box(centers);
  if (!b) return undefined;
  const out: [number, number, number, number] = [
    Math.max(b[0], clip[0] as number),
    Math.max(b[1], clip[1] as number),
    Math.min(b[2], clip[2] as number),
    Math.min(b[3], clip[3] as number),
  ];
  return out[0] <= out[2] && out[1] <= out[3] ? out : undefined;
}

/**
 * The plan of Mercator tile `z`/`x`/`y` (`size` pixels a side) from a source of URL template `url` in grid `g`; null
 * where the source has nothing
 */
export function planTile(g: Resolved, url: string, z: number, x: number, y: number, size: number): Job | null {
  const tiles = 2 ** z;
  if (g.bounds) {
    const [w, s, e, n] = g.bounds;
    if (longitude((x + 1) / tiles) <= w || longitude(x / tiles) >= e) return null;
    if (latitude(y / tiles) <= s || latitude((y + 1) / tiles) >= n) return null;
  }
  // The mesh in the source CRS
  const points = new Float64Array(2 * (N + 1) ** 2);
  for (let j = 0; j <= N; j += 1) {
    const lat = latitude((y + j / N) / tiles);
    for (let i = 0; i <= N; i += 1) {
      const [px = Number.NaN, py = Number.NaN] = g.crs.forward([longitude((x + i / N) / tiles), lat]);
      points[2 * (j * (N + 1) + i)] = px;
      points[2 * (j * (N + 1) + i) + 1] = py;
    }
  }
  let b = box(points);
  if (!b) return null;
  if (g.extent) {
    const e = g.extent;
    b = [Math.max(b[0], e[0]), Math.max(b[1], e[1]), Math.min(b[2], e[2]), Math.min(b[3], e[3])];
    if (b[0] >= b[2] || b[1] >= b[3]) return null;
  }
  const mesh = new Float32Array(points.length);
  if (g.matrices.length === 0) {
    // One image of the tile's box, a pixel larger on each side
    const [sx, sy] = pixelSize(points, size);
    const m = Number.isFinite(sx + sy) ? Math.max(sx, sy) : 0;
    const [x0, y0, x1, y1] = [b[0] - m, b[1] - m, b[2] + m, b[3] + m];
    for (let k = 0; k < points.length; k += 2) {
      mesh[k] = (((points[k] as number) - x0) / (x1 - x0)) * size;
      mesh[k + 1] = ((y1 - (points[k + 1] as number)) / (y1 - y0)) * size;
    }
    const source = { url: sourceUrl(url, z, x, y, [x0, y0, x1, y1], size, size), x: 0, y: 0, w: size, h: size };
    return { size, width: size, height: size, sources: [source], n: N, mesh };
  }
  // The level of the same pixel size, along the axis the grid stretches least (up to MAX_BOOST levels finer)
  const [sx, sy] = pixelSize(points, size);
  const across = Math.max(sx, sy);
  const target = Number.isFinite(across) ? Math.max(Math.min(sx, sy), across / 2 ** MAX_BOOST) : (b[2] - b[0]) / size;
  // Where the pixels of the tile are sampled: the centers of its edge pixels and the mesh, within the source's extent
  const centers = sampled(points, size, b);
  if (!centers) return null;
  let best = 0;
  g.matrices.forEach((m, i) => {
    const r = g.matrices[best]?.resolution ?? m.resolution;
    if (Math.abs(Math.log2(m.resolution / target)) < Math.abs(Math.log2(r / target))) best = i;
  });
  for (let level = best; level >= 0; level -= 1) {
    const m = g.matrices[level] as Matrix;
    const [ox, oy] = m.origin;
    const [tw, th, res] = [m.tileWidth, m.tileHeight, m.resolution];
    const [l0, r0, l1, r1] = m.limits ?? [0, 0, Infinity, Infinity];
    // The tiles of the source pixels sampled (those of a weight over a thousandth: a tile edge on a source tile edge
    // does not fetch the next source tile for nothing)
    const c0 = Math.max(0, l0, Math.floor(((centers[0] - ox) / res - 0.5 + 1e-3) / tw));
    const c1 = Math.min(m.matrixWidth - 1, l1, Math.floor(((centers[2] - ox) / res + 0.5 - 1e-3) / tw));
    const t0 = Math.max(0, r0, Math.floor(((oy - centers[3]) / res - 0.5 + 1e-3) / th));
    const t1 = Math.min(m.matrixHeight - 1, r1, Math.floor(((oy - centers[1]) / res + 0.5 - 1e-3) / th));
    if (c0 > c1 || t0 > t1) return null;
    const [width, height] = [(c1 - c0 + 1) * tw, (t1 - t0 + 1) * th];
    if (width * height > MAX_MOSAIC) continue;
    const sources: Placed[] = [];
    for (let row = t0; row <= t1; row += 1) {
      for (let col = c0; col <= c1; col += 1) {
        const bbox = [ox + col * tw * res, oy - (row + 1) * th * res, ox + (col + 1) * tw * res, oy - row * th * res];
        const placed = { x: (col - c0) * tw, y: (row - t0) * th, w: tw, h: th };
        sources.push({ url: sourceUrl(url, m.id, col, row, bbox, tw, th), ...placed });
      }
    }
    for (let k = 0; k < points.length; k += 2) {
      mesh[k] = ((points[k] as number) - ox) / res - c0 * tw;
      mesh[k + 1] = (oy - (points[k + 1] as number)) / res - t0 * th;
    }
    return { size, width, height, sources, n: N, mesh };
  }
  return null;
}

/**
 * The map zooms a grid's matrices span (the pixel size of the finest and of the coarsest, two levels coarser, where
 * the tiles still fit a mosaic), measured at the middle of its bounds, or of its coarsest matrix
 */
export function gridZooms(g: Resolved): { minZoom: number; maxZoom: number } {
  const coarse = g.matrices[0];
  const fine = g.matrices.at(-1);
  if (!coarse || !fine) return { minZoom: 0, maxZoom: 19 };
  let lonLat: readonly number[] = [0, 0];
  if (g.bounds) lonLat = [(g.bounds[0] + g.bounds[2]) / 2, (g.bounds[1] + g.bounds[3]) / 2];
  else {
    const [l0, r0, l1, r1] = coarse.limits ?? [0, 0, coarse.matrixWidth - 1, coarse.matrixHeight - 1];
    const span = coarse.resolution * coarse.tileWidth;
    const [cx, cy] = [(l0 + l1 + 1) / 2, (r0 + r1 + 1) / 2];
    if (Number.isFinite(cx + cy)) lonLat = g.crs.inverse([coarse.origin[0] + cx * span, coarse.origin[1] - cy * span]);
  }
  // CRS units per pixel of a 256 px tile at zoom 0 there
  const [mx, my] = [mercatorX(lonLat[0] ?? 0), mercatorY(lonLat[1] ?? 0)];
  const d = 1e-7;
  const p = g.crs.forward([longitude(mx), latitude(my)]);
  const q = g.crs.forward([longitude(mx + d), latitude(my)]);
  const zero = Math.hypot((q[0] ?? 0) - (p[0] ?? 0), (q[1] ?? 0) - (p[1] ?? 0)) / (d * 256);
  if (!(zero > 0)) return { minZoom: 0, maxZoom: 19 };
  return {
    minZoom: Math.max(0, Math.floor(Math.log2(zero / coarse.resolution)) - 2),
    maxZoom: Math.max(0, Math.round(Math.log2(zero / fine.resolution))),
  };
}

/** Draws a Mercator tile `z`/`x`/`y`: its image, null where the source has nothing; rejects on errors */
type Draw = (z: number, x: number, y: number, signal: AbortSignal) => Promise<ImageBitmap | null>;

/** A planned tile drawn in a worker; undefined when the map has no workers (drawn in the page) */
export type RemoteReprojection = (
  job: Job,
  tile: [z: number, x: number, y: number],
  signal: AbortSignal,
) => Promise<ImageBitmap | null> | undefined;

/**
 * A raster loader drawing Web Mercator tiles of `size` pixels from a source of URL template `url` in tile grid `grid`:
 * planned here, drawn in a worker of the map through `remote` when it can, else in the page
 */
export function gridLoader(url: string, grid: Resolved, size: number, remote?: RemoteReprojection): Draw {
  let local: ReturnType<typeof resampler>['draw'] | undefined;
  return async (z, x, y, signal) => {
    const job = planTile(grid, url, z, x, y, size);
    if (!job) return null;
    const drawn = remote?.(job, [z, x, y], signal);
    if (drawn) return drawn;
    local ??= resampler().draw;
    return local(job, signal);
  };
}
