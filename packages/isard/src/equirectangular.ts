/**
 * Equirectangular tiles (longitudes and latitudes, two 256 px tiles side by side at level 0, rows from the north)
 * reprojected into the Web Mercator tiles the engine draws: a raster loader that fetches the source tiles a Mercator
 * tile spans and draws them into it, one row of pixels at a time (along a row the latitude is constant, so each row is
 * a horizontal strip of the source, scaled). The planetary services (NASA's Trek, USGS) serve this grid only.
 */

/** Side of the source tiles, pixels */
const SOURCE = 256;
/** Source levels added at most towards the poles, where Web Mercator stretches the latitudes */
const MAX_BOOST = 3;
/** Source tiles kept decoded: the neighbours of a Mercator tile share them */
const CACHE = 96;

/** One strip drawn: from source tile (`tx`, `ty`) at the plan's level, the source rectangle (`sx`, `sy`, `sw`, one
 * row high) to the destination rectangle (`dx`, `dy`, `dw`, one row high) */
export interface Strip {
  tx: number;
  ty: number;
  sx: number;
  sy: number;
  sw: number;
  dx: number;
  dy: number;
  dw: number;
}

/** Where the pixels of a Mercator tile come from: the source level, and a strip per row and source column */
export interface Plan {
  level: number;
  strips: Strip[];
}

/** Latitude (degrees) of a Mercator y in [0, 1] (towards the south) */
const latitude = (y: number) => (Math.atan(Math.sinh(Math.PI * (1 - 2 * y))) * 180) / Math.PI;

/**
 * The strips filling Mercator tile `z`/`x`/`y` (`size` pixels a side) from equirectangular tiles of levels `minLevel`
 * to `maxLevel`. The source level has the same pixel size along the parallels (one less than `z`: the source is twice
 * as wide at level 0), finer towards the poles, where the Mercator pixels shrink along the meridians.
 */
export function plan(z: number, x: number, y: number, size: number, maxLevel: number, minLevel = 0): Plan {
  const n = 2 ** z;
  const center = latitude((y + 0.5) / n);
  const boost = Math.min(MAX_BOOST, Math.max(0, Math.round(-Math.log2(Math.cos((center * Math.PI) / 180)))));
  const level = Math.max(0, minLevel, Math.min(maxLevel, z - 1 + boost));
  // The source in pixels at that level: 360° across, 180° down
  const width = 2 * SOURCE * 2 ** level;
  const height = SOURCE * 2 ** level;
  const west = (x / n) * width;
  const east = ((x + 1) / n) * width;
  const scale = size / (east - west);
  const strips: Strip[] = [];
  for (let row = 0; row < size; row += 1) {
    const lat = latitude((y + (row + 0.5) / size) / n);
    const v = Math.max(0, Math.min(height - 1e-6, ((90 - lat) / 180) * height));
    const ty = Math.floor(v / SOURCE);
    // The source row centered on the destination row's latitude, kept inside its tile (no bleeding into the next)
    const sy = Math.max(0, Math.min(SOURCE - 1, v - ty * SOURCE - 0.5));
    for (let tx = Math.floor(west / SOURCE); tx * SOURCE < east; tx += 1) {
      const from = Math.max(west, tx * SOURCE);
      const to = Math.min(east, (tx + 1) * SOURCE);
      strips.push({
        tx,
        ty,
        sx: from - tx * SOURCE,
        sy,
        sw: to - from,
        dx: (from - west) * scale,
        dy: row,
        dw: (to - from) * scale,
      });
    }
  }
  return { level, strips };
}

/** A tile URL of the source: {z}, {x} and {y} replaced */
const tileUrl = (template: string, z: number, x: number, y: number) =>
  template.replaceAll('{z}', String(z)).replaceAll('{x}', String(x)).replaceAll('{y}', String(y));

/**
 * A raster loader drawing Web Mercator tiles of `size` pixels from the equirectangular tiles of `url` ({z}, {x}, {y};
 * levels `minLevel` to `maxLevel`). A missing source tile (404) leaves its part empty; a tile with no source at all is
 * missing; other errors reject (retried by the engine).
 */
export function equirectangularLoader(
  url: string,
  maxLevel: number,
  size: number,
  minLevel = 0,
): (z: number, x: number, y: number, signal: AbortSignal) => Promise<ImageBitmap | null> {
  // Source tiles by key, oldest first: a request is shared by the tiles that need it, its signal is not theirs
  const cache = new Map<string, Promise<ImageBitmap | null>>();
  /** Tiles being drawn from each source tile: an evicted one is closed once none is */
  const users = new Map<Promise<ImageBitmap | null>, number>();
  const evicted = new Set<Promise<ImageBitmap | null>>();
  const close = (tile: Promise<ImageBitmap | null>) => {
    evicted.delete(tile);
    tile.then(
      (image) => image?.close(),
      () => undefined,
    );
  };
  const release = (tile: Promise<ImageBitmap | null>) => {
    const count = (users.get(tile) ?? 1) - 1;
    if (count > 0) users.set(tile, count);
    else {
      users.delete(tile);
      if (evicted.has(tile)) close(tile);
    }
  };
  const source = (z: number, x: number, y: number): Promise<ImageBitmap | null> => {
    const key = `${z}/${x}/${y}`;
    let tile = cache.get(key);
    if (tile) {
      cache.delete(key);
    } else {
      tile = (async () => {
        const response = await fetch(tileUrl(url, z, x, y));
        if (response.status === 404 || response.status === 204 || response.status === 410) return null;
        if (!response.ok) throw new Error(`isard: ${response.status} for an equirectangular tile`);
        return createImageBitmap(await response.blob());
      })();
      // A failure is not kept: the engine's retry fetches again
      tile.catch(() => cache.delete(key));
    }
    cache.set(key, tile);
    users.set(tile, (users.get(tile) ?? 0) + 1);
    for (const [old, image] of cache) {
      if (cache.size <= CACHE) break;
      cache.delete(old);
      // Its image freed now, or once the tiles drawing from it are done
      if (users.has(image)) evicted.add(image);
      else close(image);
    }
    return tile;
  };
  return async (z, x, y, signal) => {
    const { level, strips } = plan(z, x, y, size, maxLevel, minLevel);
    const keys = [...new Set(strips.map((s) => `${s.tx}/${s.ty}`))];
    const tiles = keys.map((key) => {
      const [tx, ty] = key.split('/').map(Number) as [number, number];
      return [key, source(level, tx, ty)] as const;
    });
    try {
      const images = new Map(await Promise.all(tiles.map(async ([key, tile]) => [key, await tile] as const)));
      signal.throwIfAborted();
      if (![...images.values()].some(Boolean)) return null;
      const canvas = new OffscreenCanvas(size, size);
      const context = canvas.getContext('2d');
      if (!context) throw new Error('isard: no 2D canvas to reproject the tiles');
      for (const s of strips) {
        const image = images.get(`${s.tx}/${s.ty}`);
        if (image) context.drawImage(image, s.sx, s.sy, s.sw, 1, s.dx, s.dy, s.dw, 1);
      }
      return canvas.transferToImageBitmap();
    } finally {
      for (const [, tile] of tiles) release(tile);
    }
  };
}
