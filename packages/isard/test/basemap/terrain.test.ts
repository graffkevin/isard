import { afterAll, describe, expect, mock, test } from 'bun:test';
import { decodeHeights, encodedTerrainLoader } from '../../src/basemap/terrain.ts';
import { setGlobals } from '../fakes.ts';

describe('decodeHeights', () => {
  test('Terrarium: R × 256 + G + B / 256 - 32768', () => {
    // 0 m, Mont Blanc (4808 m: 32768 + 4808 = 37576 = 146 × 256 + 200), the floor of an ocean
    const rgba = Uint8Array.of(128, 0, 0, 255, 146, 200, 128, 255, 100, 0, 0, 255);

    expect([...decodeHeights(rgba, 'terrarium')]).toEqual([0, 4808.5, -7168]);
    // The sea floor flattened at a floor
    expect([...decodeHeights(rgba, 'terrarium', 0)]).toEqual([0, 4808.5, 0]);
  });

  test('Terrain-RGB: (R × 65536 + G × 256 + B) / 10 - 10000', () => {
    // 0 m is 100000 = 1 × 65536 + 134 × 256 + 160
    const rgba = Uint8Array.of(1, 134, 160, 255, 1, 134, 170, 255);
    const h = decodeHeights(rgba, 'mapbox');

    expect(h[0]).toBeCloseTo(0, 3);
    expect(h[1]).toBeCloseTo(1, 3);
    expect([...decodeHeights(rgba, 'mapbox', 0.5)].map((v) => Math.round(v * 10) / 10)).toEqual([0.5, 1]);
  });
});

describe('encodedTerrainLoader', () => {
  const fetched: string[] = [];
  let closed = 0;
  let failContext = false;
  /** Every pixel the floor of an ocean (-7168 m), but the first one: 4808.5 m */
  const pixels = new Uint8ClampedArray(256 * 256 * 4);

  for (let i = 0; i < pixels.length; i += 4) pixels.set([100, 0, 0, 255], i);
  pixels.set([146, 200, 128, 255], 0);

  const restore = setGlobals({
    fetch: mock(async (url: string) => {
      fetched.push(url);

      return new Response(url.includes('/9/') ? null : 'png', { status: url.includes('/9/') ? 404 : 200 });
    }),
    createImageBitmap: async () => ({
      close: () => {
        closed += 1;
      },
    }),
    OffscreenCanvas: class {
      getContext() {
        if (failContext) return null;

        return { clearRect() {}, drawImage() {}, getImageData: () => ({ data: pixels }) };
      }
    },
  });

  afterAll(restore);

  const signal = new AbortController().signal;

  test('a tile decoded below its floor; every placeholder of the URL replaced; the image freed', async () => {
    const loader = encodedTerrainLoader('https://dem.example/{z}/{x}/{y}.png?level={z}', 'terrarium', 0);
    const heights = await loader(3, 4, 5, signal);

    expect(fetched.at(-1)).toBe('https://dem.example/3/4/5.png?level=3');
    expect(heights?.length).toBe(256 * 256);
    expect(heights?.[0]).toBe(4808.5);
    // The sea floor at sea level
    expect(heights?.[1]).toBe(0);
    expect(closed).toBe(1);
  });

  test('no tile there (404): null; a failure frees the image too', async () => {
    const loader = encodedTerrainLoader('https://dem.example/{z}/{x}/{y}.png', 'terrarium');

    expect(await loader(9, 1, 1, signal)).toBeNull();
    failContext = true;
    await expect(loader(3, 1, 1, signal)).rejects.toThrow('no 2D context');
    failContext = false;
    expect(closed).toBe(2);
  });
});
