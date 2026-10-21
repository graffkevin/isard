import { afterAll, describe, expect, mock, test } from 'bun:test';
import { equirectangularGrid, gridLoader, gridZooms, planTile, resolveGrid } from '../../src/basemap/grid.ts';
import { type Job, resample, resampler } from '../../src/basemap/resample.ts';
import { getCrs } from '../../src/data/crs.ts';
import { geoplateformeWms, geoplateformeWmts, lambert93Grid } from '../../src/geoplateforme.ts';
import { latitude, longitude } from '../../src/util/geo.ts';
import { setGlobals } from '../fakes.ts';

/** The Géoplateforme's Lambert-93 pyramid (2154_10cm_6_20) */
const L93 = resolveGrid(lambert93Grid('2154_10cm_6_20', 6, 20));
const EQ = resolveGrid(equirectangularGrid(0, 8));

/** The column, row and level of each source of a job, from URLs `{z}/{x}/{y}` */
const tilesOf = (job: Job | null) => job?.sources.map((s) => s.url.split('/').map(Number)) ?? [];

/** The largest distance (source pixels) between a job's mesh, interpolated at the tile's pixel centers, and the exact
 * projection of those centers */
function meshError(g: ReturnType<typeof resolveGrid>, z: number, x: number, y: number): number {
  const job = planTile(g, '{z}/{x}/{y}', z, x, y, 256) as Job;
  const [level, col0, row0] = tilesOf(job)[0] as number[];
  const m = g.matrices.find((matrix) => Number(matrix.id) === level);
  if (!m) throw new Error('no matrix');
  const { n, mesh } = job;
  const at = (i: number, j: number, k: number) => mesh[2 * (j * (n + 1) + i) + k] as number;
  let worst = 0;
  for (let row = 0; row < 256; row += 3) {
    for (let col = 0; col < 256; col += 3) {
      const [fx, fy] = [((col + 0.5) / 256) * n, ((row + 0.5) / 256) * n];
      const [i, j] = [Math.min(n - 1, Math.floor(fx)), Math.min(n - 1, Math.floor(fy))];
      const [tx, ty] = [fx - i, fy - j];
      const lerp = (k: number) =>
        (1 - tx) * (1 - ty) * at(i, j, k) +
        tx * (1 - ty) * at(i + 1, j, k) +
        (1 - tx) * ty * at(i, j + 1, k) +
        tx * ty * at(i + 1, j + 1, k);
      const tiles = 2 ** z;
      const [X = 0, Y = 0] = g.crs.forward([
        longitude((x + (col + 0.5) / 256) / tiles),
        latitude((y + (row + 0.5) / 256) / tiles),
      ]);
      const u = (X - m.origin[0]) / m.resolution - (col0 as number) * m.tileWidth;
      const v = (m.origin[1] - Y) / m.resolution - (row0 as number) * m.tileHeight;
      worst = Math.max(worst, Math.hypot(u - lerp(0), v - lerp(1)));
    }
  }
  return worst;
}

describe('planTile in Lambert-93', () => {
  test('the level of the same pixel size, the source tiles under the Mercator tile', () => {
    // Paris at zoom 12: Mercator pixels of 25 m there, level 13 (25.6 m)
    const job = planTile(L93, '{z}/{x}/{y}', 12, 2074, 1409, 256);
    expect(tilesOf(job)).toEqual([
      [13, 98, 783],
      [13, 99, 783],
      [13, 98, 784],
      [13, 99, 784],
    ]);
    expect([job?.width, job?.height, job?.n]).toEqual([512, 512, 16]);
    expect(job?.sources[3]).toMatchObject({ x: 256, y: 256, w: 256, h: 256 });
    // The corners of the tile, in the mosaic: the Mercator tile's own corner, projected
    const [X = 0, Y = 0] = getCrs('EPSG:2154').forward([longitude(2074 / 4096), latitude(1409 / 4096)]);
    expect(job?.mesh[0]).toBeCloseTo(X / 25.6 - 98 * 256, 3);
    expect(job?.mesh[1]).toBeCloseTo((12000000 - Y) / 25.6 - 783 * 256, 3);
    // One level finer per zoom
    expect(tilesOf(planTile(L93, '{z}/{x}/{y}', 13, 4148, 2818, 256))[0]?.[0]).toBe(14);
  });

  test('the mesh within a hundredth of a source pixel of the exact projection from zoom 5, a third at zoom 3', () => {
    expect(meshError(L93, 3, 4, 2)).toBeLessThan(0.3);
    expect(meshError(L93, 5, 16, 11)).toBeLessThan(0.02);
    expect(meshError(L93, 8, 129, 88)).toBeLessThan(0.003);
    expect(meshError(L93, 12, 2074, 1409)).toBeLessThan(0.001);
  });

  test('nothing outside the bounds, nothing past the finest level, coarser levels for large tiles', () => {
    // New York
    expect(planTile(L93, '{z}/{x}/{y}', 10, 301, 385, 256)).toBeNull();
    // Zoom 22 over Paris: level 20, the last
    expect(tilesOf(planTile(L93, '{z}/{x}/{y}', 22, 2074 << 10, 1409 << 10, 256))[0]?.[0]).toBe(20);
    // France on one tile of zoom 3: the coarsest level, its tiles within the mosaic's limit
    const coarse = planTile(L93, '{z}/{x}/{y}', 3, 4, 2, 256);
    expect(tilesOf(coarse)[0]?.[0]).toBe(6);
    expect((coarse?.width ?? 0) * (coarse?.height ?? 0)).toBeLessThanOrEqual(2048 * 2048);
  });

  test('the limits of a matrix: no tile asked for outside them', () => {
    const grid = resolveGrid({
      crs: 'EPSG:2154',
      matrices: [{ id: 'a', resolution: 25.6, origin: [0, 12000000], limits: [99, 0, 200, 783] }],
    });
    expect(tilesOf(planTile(grid, '{z}/{x}/{y}', 12, 2074, 1409, 256)).map((t) => t.slice(1))).toEqual([[99, 783]]);
  });

  test('the zooms a grid spans: its finest level, its coarsest two zooms out', () => {
    expect(gridZooms(L93)).toEqual({ minZoom: 3, maxZoom: 19 });
    expect(gridZooms(resolveGrid({ crs: 'EPSG:2154' }))).toEqual({ minZoom: 0, maxZoom: 19 });
  });
});

describe('planTile of a WMS (no matrices)', () => {
  test("one image of the tile's box in the CRS, at the tile's size, a pixel larger", () => {
    const grid = resolveGrid({ crs: 'EPSG:2154' });
    const job = planTile(grid, 'wms?BBOX={bbox}&WIDTH={width}&HEIGHT={height}', 14, 8297, 5636, 512);
    const url = new URL(job?.sources[0]?.url ?? '', 'https://x/');
    const [x0, y0, x1, y1] = (url.searchParams.get('BBOX') ?? '').split(',').map(Number) as number[];
    expect([url.searchParams.get('WIDTH'), url.searchParams.get('HEIGHT')]).toEqual(['512', '512']);
    // Some 1.6 km across at 48.85° N, around the Panthéon
    expect((x1 as number) - (x0 as number)).toBeGreaterThan(1600);
    expect((x1 as number) - (x0 as number)).toBeLessThan(1700);
    expect(x0).toBeLessThan(652011);
    expect(y1).toBeGreaterThan(6860882);
    expect(job).toMatchObject({ width: 512, height: 512, sources: [{ x: 0, y: 0, w: 512, h: 512 }] });
    // The mesh inside the image
    expect(Math.min(...(job?.mesh ?? []))).toBeGreaterThan(0);
    expect(Math.max(...(job?.mesh ?? []))).toBeLessThan(512);
    expect(y0).toBeLessThan(y1 as number);
  });
});

describe('the equirectangular preset', () => {
  test('the whole world at zoom 0: both tiles of level 0', () => {
    expect(tilesOf(planTile(EQ, '{z}/{x}/{y}', 0, 0, 0, 256))).toEqual([
      [0, 0, 0],
      [0, 1, 0],
    ]);
  });

  test('the same pixel size along the parallels: level z - 1, one source column per Mercator tile', () => {
    // On the equator, east of the meridian; its source rows: the northern half of row 3 (2 boundaries: 2 rows)
    const job = planTile(EQ, '{z}/{x}/{y}', 4, 9, 7, 256);
    expect(new Set(tilesOf(job).map(([level, col]) => `${level}/${col}`))).toEqual(new Set(['3/9']));
    expect(meshError(EQ, 4, 9, 7)).toBeLessThan(0.02);
  });

  test('finer towards the poles (three levels at most), never past the last level nor before the first', () => {
    // Around 80° north: level 3 + 3, the eight source columns of the tile
    const polar = planTile(EQ, '{z}/{x}/{y}', 4, 3, 1, 256);
    expect(tilesOf(polar)[0]?.[0]).toBe(6);
    expect(new Set(tilesOf(polar).map(([, col]) => col))).toEqual(new Set([24, 25, 26, 27, 28, 29, 30, 31]));
    expect(tilesOf(planTile(resolveGrid(equirectangularGrid(0, 5)), '{z}/{x}/{y}', 4, 3, 1, 256))[0]?.[0]).toBe(5);
    expect(tilesOf(planTile(EQ, '{z}/{x}/{y}', 12, 0, 0, 256))[0]?.[0]).toBe(8);
    expect(tilesOf(planTile(resolveGrid(equirectangularGrid(2, 8)), '{z}/{x}/{y}', 2, 1, 1, 256))[0]?.[0]).toBe(2);
    // Within a source pixel at zoom 0, a tenth from zoom 4
    expect(meshError(EQ, 0, 0, 0)).toBeLessThan(1);
    expect(meshError(EQ, 4, 3, 1)).toBeLessThan(0.1);
  });
});

describe('resample', () => {
  /** A mosaic of w × h pixels, each `[r, g, b, a]` of `color(x, y)` */
  const mosaic = (w: number, h: number, color: (x: number, y: number) => number[]) => {
    const data = new Uint8ClampedArray(w * h * 4);
    for (let y = 0; y < h; y += 1) for (let x = 0; x < w; x += 1) data.set(color(x, y), 4 * (y * w + x));
    return data;
  };
  /** The mesh of an affine map: tile pixel (x, y) to mosaic (a x + b, c y + d) */
  const affine = (n: number, size: number, [a, b, c, d]: number[]) => {
    const mesh = new Float32Array(2 * (n + 1) ** 2);
    for (let j = 0; j <= n; j += 1) {
      for (let i = 0; i <= n; i += 1) {
        mesh[2 * (j * (n + 1) + i)] = ((a as number) * i * size) / n + (b as number);
        mesh[2 * (j * (n + 1) + i) + 1] = ((c as number) * j * size) / n + (d as number);
      }
    }
    return mesh;
  };

  test('one to one: the pixels as they are', () => {
    const source = mosaic(8, 8, (x, y) => [x * 30, y * 30, 7, 255]);
    expect(resample(source, 8, 8, affine(4, 8, [1, 0, 1, 0]), 4, 8)).toEqual(source);
  });

  test('an offset of half a pixel: the mean of two, a quarter of four', () => {
    const source = mosaic(4, 4, (x) => [x * 100, 0, 0, 255]);
    const out = resample(source, 4, 4, affine(2, 2, [1, 0.5, 1, 0]), 2, 2);
    expect([...out.slice(0, 4)]).toEqual([50, 0, 0, 255]);
    expect([...out.slice(4, 8)]).toEqual([150, 0, 0, 255]);
  });

  test('outside the mosaic and on NaN: transparent; colors blended with their alpha', () => {
    const source = mosaic(2, 1, (x) => (x === 0 ? [255, 0, 0, 255] : [0, 0, 255, 0]));
    // The middle of the two pixels: red at half opacity, not purple
    const out = resample(source, 2, 1, affine(1, 1, [0, 1, 0, 0.5]), 1, 1);
    expect([...out]).toEqual([255, 0, 0, 128]);
    expect([...resample(source, 2, 1, affine(1, 1, [0, 10, 0, 0.5]), 1, 1)]).toEqual([0, 0, 0, 0]);
    expect([...resample(source, 2, 1, Float32Array.of(Number.NaN, 0, 1, 0, 0, 1, 1, 1), 1, 1)]).toEqual([0, 0, 0, 0]);
  });
});

describe('resampler and gridLoader', () => {
  const fetched: string[] = [];
  const drawn: unknown[][] = [];
  class FakeCanvas {
    constructor(
      readonly width: number,
      readonly height: number,
    ) {}
    getContext() {
      return {
        // The state of the image when drawn, last
        drawImage: (...args: unknown[]) => drawn.push([...args, (args[0] as { closed?: boolean }).closed]),
        getImageData: (_x: number, _y: number, w: number, h: number) => ({ data: new Uint8ClampedArray(w * h * 4) }),
      };
    }
  }
  /** Downloads held until their promise resolves */
  const gates = new Map<string, Promise<void>>();
  const restore = setGlobals({
    fetch: mock(async (url: string) => {
      fetched.push(url);
      await gates.get(url);
      const missing = url.includes('missing') || url.endsWith('/1/0');
      return new Response(missing ? null : 'png', { status: missing ? 404 : 200 });
    }),
    createImageBitmap: async (source: Blob | { width: number }, options?: unknown) => {
      if (!(source instanceof Blob)) return { tile: source.width, options };
      const image = {
        source: await source.text(),
        closed: false,
        close() {
          image.closed = true;
        },
      };
      return image;
    },
    OffscreenCanvas: FakeCanvas,
    ImageData: class {
      constructor(
        readonly data: Uint8ClampedArray,
        readonly width: number,
      ) {}
    },
  });
  afterAll(restore);
  const signal = new AbortController().signal;

  test('the source tiles fetched once, missing ones left empty; nothing outside the source', async () => {
    const loader = gridLoader('https://eq.example/{z}/{x}/{y}', EQ, 256);
    expect(await loader(0, 0, 0, signal)).toEqual({ tile: 256, options: { premultiplyAlpha: 'none' } } as never);
    expect(fetched.sort()).toEqual(['https://eq.example/0/0/0', 'https://eq.example/0/1/0']);
    // Only the western tile drawn, at its place
    expect(drawn.map((d) => d.slice(1, 5))).toEqual([[0, 0, 256, 256]]);
    await loader(0, 0, 0, signal);
    expect(fetched.length).toBe(2);
    expect(await gridLoader('https://missing.example/{z}/{x}/{y}', EQ, 256)(3, 4, 3, signal)).toBeNull();
    // Outside France: not even planned
    fetched.length = 0;
    expect(await gridLoader('https://l93.example/{z}/{x}/{y}', L93, 256)(10, 301, 385, signal)).toBeNull();
    expect(fetched).toEqual([]);
  });

  test('planned in the page, drawn by a worker when the map has one', async () => {
    const jobs: Job[] = [];
    const remote = (job: Job) => {
      jobs.push(job);
      return Promise.resolve(null);
    };
    await gridLoader('https://l93.example/{z}/{x}/{y}', L93, 512, remote)(12, 2074, 1409, signal);
    // Tiles of 512 pixels: one level finer
    expect(jobs[0]?.size).toBe(512);
    expect(jobs[0]?.sources[0]?.url).toStartWith('https://l93.example/14/');
  });

  test('source images past the cache are freed; one still in use once drawn', async () => {
    const draw = resampler(8).draw;
    const job = (k: number): Job => ({
      size: 1,
      width: 256,
      height: 256,
      n: 1,
      mesh: new Float32Array(8),
      sources: [{ url: `https://evict.example/${k}`, x: 0, y: 0, w: 256, h: 256 }],
    });
    let open = () => {};
    gates.set(
      'https://evict.example/0',
      new Promise((resolve) => {
        open = resolve;
      }),
    );
    drawn.length = 0;
    const held = draw(job(0), signal);
    for (let k = 1; k <= 20; k += 1) await draw(job(k), signal);
    await new Promise((resolve) => setTimeout(resolve, 0));
    const imageOf = (entry: unknown[] | undefined) => entry?.[0] as { closed: boolean };
    // The first drawn, from an open image, closed since it left the cache; the latest ones kept
    expect(drawn[0]?.at(-1)).toBe(false);
    expect(imageOf(drawn[0]).closed).toBe(true);
    expect(imageOf(drawn.at(-1)).closed).toBe(false);
    // The held one, evicted meanwhile, is drawn open, then freed
    open();
    await held;
    expect(drawn.at(-1)?.at(-1)).toBe(false);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(imageOf(drawn.at(-1)).closed).toBe(true);
  });

  test('an aborted request rejects', async () => {
    const controller = new AbortController();
    controller.abort();
    expect(gridLoader('https://eq.example/{z}/{x}/{y}', EQ, 256)(0, 0, 0, controller.signal)).rejects.toThrow();
  });
});

describe("the Géoplateforme's Lambert-93 layers", () => {
  test('a WMTS layer in a Lambert-93 matrix set: its grid, the set in the URL', () => {
    const options = geoplateformeWmts('GEOGRAPHICALGRIDSYSTEMS.PLANIGNV2.L93', {
      matrixSet: '2154_10cm_6_20',
      format: 'image/jpeg',
    });
    expect(options.url).toContain('TILEMATRIXSET=2154_10cm_6_20&TILEMATRIX={z}&TILEROW={y}&TILECOL={x}');
    expect(options.maxZoom).toBeUndefined();
    const grid = options.grid as { matrices: { id: number; resolution: number }[]; crs: string };
    expect(grid.crs).toBe('EPSG:2154');
    expect(grid.matrices[0]).toMatchObject({ id: 6, resolution: 3276.8 });
    expect(grid.matrices.at(-1)).toMatchObject({ id: 20, resolution: 0.2 });
    // A set it cannot make: wmtsGrid says
    expect(() => geoplateformeWmts('INRAE_CARTE-SOLS', { matrixSet: '2154_10m_0_14' })).toThrow('wmtsGrid');
    expect(geoplateformeWmts('ORTHOIMAGERY.ORTHOPHOTOS')).toMatchObject({ maxZoom: 19 });
  });

  test('a WMS asked for in Lambert-93, or in CRS:84 for EPSG:4326', () => {
    const options = geoplateformeWms('CADASTRALPARCELS.PARCELLAIRE_EXPRESS', { crs: 'EPSG:2154' });
    expect(options.url).toContain('CRS=EPSG:2154&BBOX={bbox}&WIDTH={width}&HEIGHT={height}');
    expect(options.grid).toMatchObject({ crs: 'EPSG:2154', bounds: [-5.5, 41, 10, 51.5] });
    expect(geoplateformeWms('X', { crs: 'EPSG:4326' }).url).toContain('CRS=CRS:84');
    expect(geoplateformeWms('X').url).toContain('CRS=EPSG:3857&BBOX={bbox}&WIDTH=256&HEIGHT=256');
  });
});
