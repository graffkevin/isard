import { afterEach, describe, expect, test } from 'bun:test';
import { geoplateformeWfs } from '../src/geoplateforme.ts';

const original = globalThis.fetch;
afterEach(() => {
  globalThis.fetch = original;
});

/** A WFS of `matched` features numbered from 0, answering pages as the Géoplateforme does (5000 at most) */
const serve = (matched: number, asked: string[]) => {
  globalThis.fetch = (async (input: URL) => {
    const start = Number(input.searchParams.get('STARTINDEX') ?? 0);
    const count = Math.min(Number(input.searchParams.get('COUNT')), 5000);

    asked.push(`${start}+${count}`);

    const features = Array.from({ length: Math.max(0, Math.min(count, matched - start)) }, (_, i) => ({
      type: 'Feature',
      id: `batiment.${start + i}`,
      properties: {},
      geometry: null,
    }));

    return Response.json({ type: 'FeatureCollection', numberMatched: matched, features });
  }) as unknown as typeof fetch;
};

describe('geoplateformeWfs', () => {
  test('past one page: the others asked together after the first, each feature once', async () => {
    const asked: string[] = [];

    serve(12_345, asked);

    const collection = await geoplateformeWfs('BDTOPO_V3:batiment', [2.33, 48.87, 2.36, 48.89]);

    expect(asked).toEqual(['0+5000', '5000+5000', '10000+2345']);
    expect(collection.features.length).toBe(12_345);
    expect(new Set(collection.features.map((f) => f.id)).size).toBe(12_345);
  });

  test('one page when it holds them all; at most `count` features', async () => {
    const asked: string[] = [];

    serve(800, asked);
    expect((await geoplateformeWfs('BDTOPO_V3:batiment', [2.33, 48.87, 2.36, 48.89])).features.length).toBe(800);
    expect(asked).toEqual(['0+5000']);
    asked.length = 0;
    serve(20_000, asked);

    const capped = await geoplateformeWfs('BDTOPO_V3:batiment', [2.33, 48.87, 2.36, 48.89], { count: 7000 });

    expect(capped.features.length).toBe(7000);
    expect(asked).toEqual(['0+5000', '5000+2000']);
  });
});
