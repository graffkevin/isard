/** PMTiles v3: tile ids on the Hilbert curve, directories, the committed fixture (gzip, leaf directories), runs of
 * tiles, ranges over HTTP; a public archive with `ISARD_LIVE=1` */
import { afterAll, describe, expect, test } from 'bun:test';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import {
  findEntry,
  PMTiles,
  parseDirectory,
  parseHeader,
  pmtilesTile,
  pmtilesUrl,
  tileIdToZxy,
  zxyToTileId,
} from '../../src/basemap/pmtiles.ts';
import { basicStyle } from '../../src/basemap/vectortiles.ts';
import { varint, writePmtiles } from '../pmtiles-writer.ts';

const fixture = readFileSync(join(import.meta.dir, '../fixtures/tiny.pmtiles'));

describe('tile ids', () => {
  test('the levels and the Hilbert curve, as the specification numbers them', () => {
    expect(zxyToTileId(0, 0, 0)).toBe(0);
    expect(zxyToTileId(1, 0, 0)).toBe(1);
    expect(zxyToTileId(1, 0, 1)).toBe(2);
    expect(zxyToTileId(1, 1, 1)).toBe(3);
    expect(zxyToTileId(1, 1, 0)).toBe(4);
    expect(zxyToTileId(2, 0, 0)).toBe(5);
    expect(zxyToTileId(12, 3423, 1763)).toBe(19078479);
    expect(tileIdToZxy(19078479)).toEqual([12, 3423, 1763]);
  });

  test('round trips up to level 26, past 32 bits', () => {
    (
      [
        [3, 5, 2],
        [17, 66_000, 45_000],
        [26, 2 ** 26 - 1, 12_345_678],
        [26, 0, 2 ** 26 - 1],
      ] as [number, number, number][]
    ).forEach(([z, x, y]) => {
      expect(tileIdToZxy(zxyToTileId(z, x, y))).toEqual([z, x, y]);
    });
    expect(() => zxyToTileId(27, 0, 0)).toThrow();
    expect(() => zxyToTileId(2, 4, 0)).toThrow();
  });
});

describe('directories', () => {
  test('ids by deltas, offsets following their previous entry', () => {
    const bytes = new Uint8Array([
      ...varint(3),
      ...[5, 1, 2_000_000_000_000].flatMap(varint),
      ...[1, 3, 0].flatMap(varint),
      ...[10, 20, 30].flatMap(varint),
      ...[0 + 1, 0, 100 + 1].flatMap(varint),
    ]);
    const entries = parseDirectory(bytes);

    expect(entries).toEqual([
      { tileId: 5, offset: 0, length: 10, runLength: 1 },
      { tileId: 6, offset: 10, length: 20, runLength: 3 },
      { tileId: 2_000_000_000_006, offset: 100, length: 30, runLength: 0 },
    ]);
    expect(findEntry(entries, 5)?.length).toBe(10);
    // Within the run of the second entry, then past it
    expect(findEntry(entries, 8)?.length).toBe(20);
    expect(findEntry(entries, 9)).toBeUndefined();
    expect(findEntry(entries, 4)).toBeUndefined();
    // A leaf directory covers every id after it
    expect(findEntry(entries, 2_000_000_000_100)?.runLength).toBe(0);
  });

  test('a truncated directory is an error', () => {
    expect(() => parseDirectory(new Uint8Array([3, 1]))).toThrow();
  });
});

describe('the fixture', () => {
  test('stays small', () => {
    expect(fixture.length).toBeLessThan(100 * 1024);
  });

  test('its header', () => {
    const header = parseHeader(fixture);

    expect(header.tileType).toBe('mvt');
    expect([header.minZoom, header.maxZoom]).toEqual([0, 3]);
    expect(header.internalCompression).toBe(2);
    expect(header.bounds.map((d) => Math.round(d * 10) / 10)).toEqual([2.2, 48.8, 2.5, 48.9]);
    expect(() => parseHeader(new Uint8Array(200))).toThrow('not a PMTiles archive');
  });

  test('its tiles through leaf directories, gunzipped; nothing elsewhere', async () => {
    const archive = new PMTiles(new Blob([fixture]));

    expect((await archive.metadata()).vector_layers).toEqual([{ id: 'places' }, { id: 'roads' }]);
    for (const [z, x, y] of [
      [0, 0, 0],
      [1, 1, 0],
      [2, 2, 1],
      [3, 4, 2],
    ] as [number, number, number][]) {
      const tile = await archive.tile(z, x, y);

      // A vector tile: its first layer's tag (field 3, bytes)
      expect(tile?.[0]).toBe(0x1a);
    }
    expect(await archive.tile(1, 0, 0)).toBeUndefined();
    expect(await archive.tile(4, 8, 5)).toBeUndefined();
  });
});

describe('archives written here', () => {
  const png = (n: number) => new Uint8Array([0x89, 0x50, 0x4e, 0x47, n]);

  test('runs of identical tiles, uncompressed', async () => {
    const tiles = [0, 1, 2, 3].map((x) => ({ z: 2, x, y: 0, data: png(x === 3 ? 9 : 1) }));
    const bytes = writePmtiles(tiles, { tileType: 'png' });
    const archive = new PMTiles(new Blob([bytes]));

    expect((await archive.header()).tileType).toBe('png');
    for (const t of tiles) expect(await archive.tile(t.z, t.x, t.y)).toEqual(t.data);
  });

  test('many tiles across leaf directories', async () => {
    const tiles = Array.from({ length: 16 }, (_, x) => x).flatMap((x) =>
      Array.from({ length: 16 }, (_, y) => ({ z: 4, x, y, data: png(x * 16 + y) })),
    );
    const archive = new PMTiles(new Blob([writePmtiles(tiles, { tileType: 'png', gzip: true, leafSize: 10 })]));

    for (const t of tiles) expect(await archive.tile(t.z, t.x, t.y)).toEqual(t.data);
  });

  test('brotli is refused with a message', async () => {
    const bytes = writePmtiles([{ z: 0, x: 0, y: 0, data: png(1) }]);

    bytes[98] = 3;
    await expect(new PMTiles(new Blob([bytes])).tile(0, 0, 0)).rejects.toThrow('brotli');
  });
});

describe('over HTTP', () => {
  const requests: (string | null)[] = [];
  let ranges = true;
  const server = Bun.serve({
    hostname: '127.0.0.1',
    port: 0,
    fetch(request) {
      const range = request.headers.get('Range');

      requests.push(range);

      const match = /bytes=(\d+)-(\d+)/.exec(range ?? '');

      if (!ranges || !match) return new Response(fixture);

      const [start, end] = [Number(match[1]), Math.min(Number(match[2]), fixture.length - 1)];

      return new Response(fixture.subarray(start, end + 1), {
        status: 206,
        headers: { 'Content-Range': `bytes ${start}-${end}/${fixture.length}` },
      });
    },
  });

  afterAll(() => server.stop(true));

  const url = `http://127.0.0.1:${server.port}/tiny.pmtiles`;

  test('by ranges: the head once, then each tile; directories kept', async () => {
    requests.length = 0;

    const archive = new PMTiles(url);

    await archive.tile(3, 4, 2);
    await archive.tile(3, 4, 2);
    // The head (16 KB), a leaf directory, then the tile twice: the leaf is kept
    expect(requests[0]).toBe('bytes=0-16383');
    expect(requests.length).toBe(4);
  });

  test('a server ignoring ranges: the whole small file kept', async () => {
    ranges = false;
    requests.length = 0;

    const archive = new PMTiles(`${url}?whole`);

    expect(await archive.tile(2, 2, 1)).toBeDefined();
    expect(await archive.tile(0, 0, 0)).toBeDefined();
    expect(requests.length).toBe(1);
    ranges = true;
  });

  test('pmtiles:// URLs: tile templates read from their archive', async () => {
    expect(pmtilesUrl('https://a.b/c.pmtiles')).toBeUndefined();
    expect(pmtilesUrl('pmtiles://files/x.pmtiles', 'http://127.0.0.1:5190/viewer/')).toBe(
      'http://127.0.0.1:5190/viewer/files/x.pmtiles',
    );
    expect(pmtilesTile('https://a.b/0/0/0')).toBe(false);

    const tile = pmtilesTile(`pmtiles://${url}/1/1/0`);

    expect(tile && (await tile)?.[0]).toBe(0x1a);
  });
});

describe('basicStyle', () => {
  test('fills, lines, circles and labels for each layer of a source', () => {
    const style = basicStyle({ url: 'pmtiles://a.pmtiles' }, ['water', 'roads']);

    expect(style.sources.data).toEqual({ type: 'vector', url: 'pmtiles://a.pmtiles' });
    expect(style.layers.map((l) => l.id)).toEqual([
      'water-fill',
      'water-line',
      'water-circle',
      'roads-fill',
      'roads-line',
      'roads-circle',
      'water-label',
      'roads-label',
    ]);
    expect(style.layers.every((l) => l.source === 'data')).toBe(true);
  });
});

const LIVE = 'https://r2-public.protomaps.com/protomaps-sample-datasets/cb_2018_us_zcta510_500k.pmtiles';

test.skipIf(!process.env.ISARD_LIVE)('a public archive (ISARD_LIVE=1): header, metadata, a tile', async () => {
  const archive = new PMTiles(LIVE);
  const header = await archive.header();

  expect(header.tileType).toBe('mvt');
  expect(((await archive.metadata()).vector_layers as { id: string }[]).map((l) => l.id)).toContain('zcta');

  // New York at level 7
  const tile = await archive.tile(7, 37, 48);

  expect(tile?.length).toBeGreaterThan(1000);
});
