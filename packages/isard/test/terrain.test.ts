import { describe, expect, test } from 'bun:test';
import { decodeHeights } from '../src/terrain.ts';

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
  });
});
