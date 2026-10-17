import { afterAll, describe, expect, mock, test } from 'bun:test';
import { equirectangularLoader, plan } from '../src/equirectangular.ts';
import { setGlobals } from './fakes.ts';

/** Latitude (degrees) of a Mercator y in [0, 1] */
const latitude = (y: number) => (Math.atan(Math.sinh(Math.PI * (1 - 2 * y))) * 180) / Math.PI;

describe('plan', () => {
  test('the whole world at zoom 0: both source tiles of level 0, side by side', () => {
    const { level, strips } = plan(0, 0, 0, 256, 8);
    expect(level).toBe(0);
    expect(strips.length).toBe(256 * 2);
    const [west, east] = strips.filter((s) => s.dy === 0);
    expect(west).toMatchObject({ tx: 0, ty: 0, sx: 0, sw: 256, dx: 0, dw: 128 });
    expect(east).toMatchObject({ tx: 1, ty: 0, sx: 0, sw: 256, dx: 128, dw: 128 });
    // From 85° north down to 85° south, the equator in the middle of the source rows
    const rows = strips.filter((s) => s.tx === 0).map((s) => s.sy);
    expect(rows[0]).toBeCloseTo(((90 - latitude(0.5 / 256)) / 180) * 256 - 0.5, 9);
    expect(rows.every((sy, i) => i === 0 || sy > (rows[i - 1] as number))).toBe(true);
    expect(rows[128]).toBeCloseTo(128 - 0.5 + ((0 - latitude(128.5 / 256)) / 180) * 256, 9);
  });

  test('the same pixel size along the parallels: level z - 1, one source column per Mercator tile', () => {
    // A tile on the equator, east of the meridian
    const { level, strips } = plan(4, 9, 7, 256, 8);
    expect(level).toBe(3);
    expect(new Set(strips.map((s) => s.tx))).toEqual(new Set([9]));
    expect(strips.every((s) => s.sx === 0 && s.sw === 256 && s.dx === 0 && s.dw === 256)).toBe(true);
    // Its rows: the latitude of each row's center
    for (const row of [0, 100, 255]) {
      const strip = strips.find((s) => s.dy === row);
      const v = ((90 - latitude((7 + (row + 0.5) / 256) / 16)) / 180) * 256 * 8;
      expect((strip?.ty ?? 0) * 256 + (strip?.sy ?? 0)).toBeCloseTo(v - 0.5, 6);
    }
  });

  test('finer towards the poles, up to the last level', () => {
    // Around 80° north, Mercator pixels are some 6 times shorter along the meridians
    const polar = plan(4, 3, 1, 256, 8);
    expect(polar.level).toBe(3 + 3);
    // The eight source columns of the tile, the boundaries on whole pixels
    expect(new Set(polar.strips.map((s) => s.tx))).toEqual(new Set([24, 25, 26, 27, 28, 29, 30, 31]));
    expect(polar.strips.every((s) => Number.isInteger(s.dx) && Number.isInteger(s.dw))).toBe(true);
    expect(plan(4, 3, 1, 256, 5).level).toBe(5);
    expect(plan(12, 0, 0, 256, 8).level).toBe(8);
  });
});

describe('equirectangularLoader', () => {
  const fetched: string[] = [];
  const drawn: unknown[][] = [];
  class FakeCanvas {
    constructor(
      readonly width: number,
      readonly height: number,
    ) {}
    getContext() {
      return { drawImage: (...args: unknown[]) => drawn.push(args) };
    }
    transferToImageBitmap() {
      return { width: this.width, height: this.height };
    }
  }
  const restore = setGlobals({
    fetch: mock(async (url: string) => {
      fetched.push(url);
      // The eastern half is missing, and everything of missing.example
      const missing = url.endsWith('/1.jpg') || url.includes('missing');
      return new Response(missing ? null : 'jpeg', { status: missing ? 404 : 200 });
    }),
    createImageBitmap: async (blob: Blob) => ({ source: await blob.text() }),
    OffscreenCanvas: FakeCanvas,
  });
  afterAll(restore);

  test('the source tiles fetched once, missing ones left empty', async () => {
    const loader = equirectangularLoader('https://trek.example/{z}/{y}/{x}.jpg', 8, 256);
    const signal = new AbortController().signal;
    const tile = await loader(0, 0, 0, signal);
    expect(tile).toEqual({ width: 256, height: 256 } as unknown as ImageBitmap);
    expect(fetched.sort()).toEqual(['https://trek.example/0/0/0.jpg', 'https://trek.example/0/0/1.jpg']);
    // Only the western half drawn, a strip per row
    expect(drawn.length).toBe(256);
    expect(drawn[0]?.slice(1, 5)).toEqual([0, expect.any(Number), 256, 1]);
    // Kept for the next tiles that need them
    await loader(0, 0, 0, signal);
    expect(fetched.length).toBe(2);
    // No source at all: no tile
    const nothing = equirectangularLoader('https://missing.example/{z}/{y}/{x}.jpg', 8, 256);
    expect(await nothing(3, 4, 3, signal)).toBeNull();
  });

  test('an aborted request rejects', async () => {
    const loader = equirectangularLoader('https://trek.example/{z}/{y}/{x}.jpg', 8, 256);
    const controller = new AbortController();
    controller.abort();
    expect(loader(0, 0, 0, controller.signal)).rejects.toThrow();
  });
});
