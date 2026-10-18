/** VectorTiles: its style resolved (TileJSON, raster sources), its layers and workers */
import { afterAll, describe, expect, mock, test } from 'bun:test';
import type { BasemapRequest } from '../src/basemap.worker.ts';
import type { GeoMap } from '../src/map.ts';
import type { Style } from '../src/vectorstyle.ts';
import { BasemapWorkers, type PoolWorker, VectorTiles } from '../src/vectortiles.ts';
import { setGlobals } from './fakes.ts';

class FakeWorker {
  messages: BasemapRequest[] = [];
  onmessage: PoolWorker['onmessage'] = null;
  onerror: PoolWorker['onerror'] = null;
  onmessageerror: PoolWorker['onmessageerror'] = null;
  postMessage(message: BasemapRequest) {
    this.messages.push(message);
  }
  terminate() {}
}

const fetchMock = mock(async (url: string) => {
  if (url === 'https://tiles.example/planet.json')
    return Response.json({ tiles: ['https://tiles.example/{z}/{x}/{y}.pbf'], maxzoom: 13 });
  return new Response(null, { status: 404 });
});
const restore = setGlobals({ fetch: fetchMock });
afterAll(restore);

/** What VectorTiles needs of a map to resolve its style: workers */
function host(workers: FakeWorker[] = []): GeoMap {
  const pool = new BasemapWorkers(2, 'worker.js', () => {
    const w = new FakeWorker();
    workers.push(w);
    return w as unknown as PoolWorker;
  });
  return { basemapWorkers: () => ({ pool, own: true }) } as unknown as GeoMap;
}

const style = (): Style => ({
  version: 8,
  sources: {
    planet: { type: 'vector', url: 'https://tiles.example/planet.json' },
    relief: { type: 'raster', tiles: ['https://relief.example/{z}/{x}/{y}.png'], tileSize: 256, maxzoom: 9 },
    dem: { type: 'raster-dem', tiles: ['https://dem.example/{z}/{x}/{y}.png'] },
  },
  layers: [
    { id: 'background', type: 'background' },
    { id: 'water', type: 'fill', source: 'planet', 'source-layer': 'water' },
    { id: 'relief', type: 'raster', source: 'relief' },
    { id: 'hills', type: 'hillshade', source: 'dem' },
    { id: 'labels', type: 'symbol', source: 'planet', 'source-layer': 'place' },
    { id: 'elsewhere', type: 'line', source: 'other', 'source-layer': 'roads' },
  ],
});

describe('VectorTiles.load', () => {
  test("a source given by URL: its TileJSON's tiles and maxzoom; raster sources kept, other layers left out", async () => {
    const input = style();
    const layers = input.layers;
    const ready = await new VectorTiles(host(), { style: input }).ready;
    expect(ready.tiles).toBe('https://tiles.example/{z}/{x}/{y}.pbf');
    expect(ready.maxzoom).toBe(13);
    expect(ready.rasters).toEqual({
      relief: { tiles: 'https://relief.example/{z}/{x}/{y}.png', maxzoom: 9, tileSize: 256 },
    });
    expect(ready.style.layers.map((l) => l.id)).toEqual(['background', 'water', 'relief', 'labels']);
    // The application's style object is not changed
    expect(input.layers).toBe(layers);
    expect(input.layers.length).toBe(6);
  });

  test('tiles given in the style: no TileJSON; 14 by default', async () => {
    const calls = fetchMock.mock.calls.length;
    const ready = await new VectorTiles(host(), {
      style: { version: 8, sources: { v: { type: 'vector', tiles: ['https://t/{z}/{x}/{y}'] } }, layers: [] },
    }).ready;
    expect(ready.tiles).toBe('https://t/{z}/{x}/{y}');
    expect(ready.maxzoom).toBe(14);
    expect(fetchMock.mock.calls.length).toBe(calls);
  });

  test('a style without a vector source fails', async () => {
    const warn = console.warn;
    console.warn = () => undefined;
    try {
      const tiles = new VectorTiles(host(), { style: { version: 8, sources: {}, layers: [] } });
      await expect(tiles.ready).rejects.toThrow('no vector source');
    } finally {
      console.warn = warn;
    }
  });
});

describe('workers', () => {
  test('a renderer tells the workers how many they are (each keeps its share of the decoded tiles)', () => {
    const workers: FakeWorker[] = [];
    const pool = new BasemapWorkers(3, 'worker.js', () => {
      const w = new FakeWorker();
      workers.push(w);
      return w as unknown as PoolWorker;
    });
    pool.broadcast({ type: 'init', key: 'v0:r0', tiles: 't', maxzoom: 14, layers: [] });
    pool.broadcast({ type: 'drop', key: 'v0:r0' });
    for (const w of workers) {
      expect(w.messages[0]).toMatchObject({ type: 'init', workers: 3 });
      expect(w.messages[1]).toEqual({ type: 'drop', key: 'v0:r0' });
    }
  });
});
