/** The local viewer's server: ranges, MBTiles rows flipped, the bridge's replay and relay, startup files, refusals */
import { Database } from 'bun:sqlite';
import { afterAll, beforeAll, describe, expect, test } from 'bun:test';
import { mkdtempSync, writeFileSync } from 'node:fs';
import { request as httpRequest } from 'node:http';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createViewerServer, type ViewerServer } from './viewer.ts';

const dir = mkdtempSync(join(tmpdir(), 'isard-viewer-'));
const root = join(dir, 'dist');
const bytes = new Uint8Array(1000).map((_, i) => i % 251);

/** The events of a Server-Sent Events stream, `count` of them */
const events = async (url: string, count: number, after?: () => Promise<unknown>): Promise<unknown[]> => {
  const controller = new AbortController();
  const response = await fetch(url, { signal: controller.signal });
  expect(response.headers.get('Content-Type')).toBe('text/event-stream');
  const reader = (response.body as ReadableStream<Uint8Array>).getReader();
  const decoder = new TextDecoder();
  const out: unknown[] = [];
  let text = '';
  let started = false;
  while (out.length < count) {
    const { value, done } = await reader.read();
    if (done) break;
    text += decoder.decode(value, { stream: true });
    // The stream is open: what the test sends next arrives on it
    if (!started && after) {
      started = true;
      await after();
    }
    for (let end = text.indexOf('\n\n'); end >= 0; end = text.indexOf('\n\n')) {
      const block = text.slice(0, end);
      text = text.slice(end + 2);
      if (block.startsWith('data: ')) out.push(JSON.parse(block.slice(6)));
    }
  }
  controller.abort();
  return out;
};

let viewer: ViewerServer;
let base: string;
const tokens: Record<string, string> = {};

beforeAll(async () => {
  await Bun.write(join(root, 'index.html'), '<!doctype html><title>viewer</title>');
  await Bun.write(join(root, 'isard_bg.wasm'), new Uint8Array([0, 97, 115, 109]));
  writeFileSync(join(dir, 'data.bin'), bytes);
  writeFileSync(join(dir, 'cloud.copc.laz'), bytes);
  writeFileSync(
    join(dir, 'places.geojson'),
    JSON.stringify({
      type: 'FeatureCollection',
      features: [
        { type: 'Feature', properties: { name: 'a' }, geometry: { type: 'Point', coordinates: [2, 48] } },
        {
          type: 'Feature',
          properties: {},
          geometry: {
            type: 'LineString',
            coordinates: [
              [3, 49],
              [4, 50],
            ],
          },
        },
      ],
    }),
  );
  // An MBTiles of vector tiles: level 1, column 1, TMS row 0 (XYZ row 1), gzipped as stored
  const db = new Database(join(dir, 'tiles.mbtiles'));
  db.run('CREATE TABLE metadata (name TEXT, value TEXT)');
  db.run('CREATE TABLE tiles (zoom_level INTEGER, tile_column INTEGER, tile_row INTEGER, tile_data BLOB)');
  for (const [name, value] of [
    ['format', 'pbf'],
    ['minzoom', '0'],
    ['maxzoom', '5'],
    ['bounds', '-10,-20,30,40'],
    ['json', JSON.stringify({ vector_layers: [{ id: 'water' }, { id: 'roads' }] })],
  ] as [string, string][]) {
    db.run('INSERT INTO metadata VALUES (?, ?)', [name, value]);
  }
  db.run('INSERT INTO tiles VALUES (1, 1, 0, ?)', [Bun.gzipSync(new Uint8Array([1, 2, 3]))]);
  db.close();
  viewer = await createViewerServer({
    port: 0,
    root,
    print: false,
    files: [
      join(dir, 'data.bin'),
      join(dir, 'cloud.copc.laz'),
      join(dir, 'places.geojson'),
      join(dir, 'tiles.mbtiles'),
      join(dir, 'missing.kml'),
    ],
  });
  base = viewer.url;
  // The tokens, from the messages replayed
  for (const m of (await events(`${base}bridge/events`, 4)) as { url?: string; name?: string }[]) {
    if (m.url && m.name) tokens[m.name] = m.url.split('/')[1] ?? '';
  }
});

afterAll(() => viewer.stop());

describe('static files', () => {
  test('the viewer page and its wasm', async () => {
    expect(await (await fetch(base)).text()).toContain('viewer');
    expect((await fetch(`${base}isard_bg.wasm`)).headers.get('Content-Type')).toBe('application/wasm');
    expect((await fetch(`${base}nothing.js`)).status).toBe(404);
  });
});

describe('ranges', () => {
  // data.bin is not a format of the viewer: not served (reported on startup)
  test('only files added are served', async () => {
    expect((await fetch(`${base}files/abc/data.bin`)).status).toBe(404);
  });

  const url = () => `${base}files/${tokens['cloud.copc.laz']}/cloud.copc.laz`;

  test('206 for a range, exactly its bytes', async () => {
    const whole = new Uint8Array(await (await fetch(url())).arrayBuffer());
    expect(whole).toEqual(bytes);
    const response = await fetch(url(), { headers: { Range: 'bytes=10-19' } });
    expect(response.status).toBe(206);
    expect(response.headers.get('Content-Range')).toBe(`bytes 10-19/${whole.length}`);
    expect(response.headers.get('Accept-Ranges')).toBe('bytes');
    expect(new Uint8Array(await response.arrayBuffer())).toEqual(whole.subarray(10, 20));
    const open = await fetch(url(), { headers: { Range: `bytes=${whole.length - 5}-` } });
    expect(new Uint8Array(await open.arrayBuffer())).toEqual(whole.subarray(whole.length - 5));
    const suffix = await fetch(url(), { headers: { Range: 'bytes=-7' } });
    expect(suffix.status).toBe(206);
    expect(new Uint8Array(await suffix.arrayBuffer())).toEqual(whole.subarray(whole.length - 7));
    // Past its end: clipped
    const past = await fetch(url(), { headers: { Range: `bytes=0-${whole.length + 100}` } });
    expect(past.headers.get('Content-Length')).toBe(String(whole.length));
  });

  test('416 past the end, HEAD without a body', async () => {
    const response = await fetch(url(), { headers: { Range: 'bytes=99999-' } });
    expect(response.status).toBe(416);
    expect(response.headers.get('Content-Range')).toMatch(/^bytes \*\/\d+$/);
    const head = await fetch(url(), { method: 'HEAD' });
    expect(head.status).toBe(200);
    expect(Number(head.headers.get('Content-Length'))).toBeGreaterThan(100);
    expect(head.headers.get('Accept-Ranges')).toBe('bytes');
    expect((await fetch(`${base}files/${tokens['cloud.copc.laz']}/other.laz`)).status).toBe(404);
  });
});

describe('MBTiles', () => {
  test('XYZ rows flipped from TMS, gzip as stored, 204 where none', async () => {
    const token = tokens['tiles.mbtiles'];
    const response = await fetch(`${base}mbtiles/${token}/1/1/1.pbf`, { decompress: false });
    expect(response.status).toBe(200);
    expect(response.headers.get('Content-Type')).toBe('application/x-protobuf');
    expect(response.headers.get('Content-Encoding')).toBe('gzip');
    const inflated = await fetch(`${base}mbtiles/${token}/1/1/1`);
    expect(new Uint8Array(await inflated.arrayBuffer())).toEqual(new Uint8Array([1, 2, 3]));
    // TMS row 0 is XYZ row 1: row 0 has nothing
    expect((await fetch(`${base}mbtiles/${token}/1/1/0`)).status).toBe(204);
    const metadata = (await (await fetch(`${base}mbtiles/${token}/metadata.json`)).json()) as Record<string, unknown>;
    expect(metadata.format).toBe('pbf');
    expect(metadata.vector_layers).toEqual([{ id: 'water' }, { id: 'roads' }]);
  });
});

describe('the bridge', () => {
  test('the startup files replayed, the view fitted to them', async () => {
    const replay = (await events(`${base}bridge/events`, 4)) as Record<string, unknown>[];
    const [fit, cloud, geojson, tiles] = replay;
    expect(cloud).toEqual({
      type: 'addPointCloud',
      id: 'cloud.copc.laz',
      name: 'cloud.copc.laz',
      url: `files/${tokens['cloud.copc.laz']}/cloud.copc.laz`,
    });
    // The camera first (the union of the files' bounds), then the layers
    expect(fit?.type).toBe('fitBounds');
    expect(fit?.bounds).toEqual([-10, -20, 30, 50]);
    expect(geojson?.type).toBe('addGeoJSON');
    expect(geojson?.id).toBe('places.geojson');
    expect(tiles?.type).toBe('addVectorTiles');
    expect(tiles?.layers).toEqual(['water', 'roads']);
    expect(tiles?.url).toBe(`mbtiles/${tokens['tiles.mbtiles']}/{z}/{x}/{y}`);
  });

  test('host messages sent, the state replayed to a new viewer', async () => {
    const post = (body: unknown) =>
      fetch(`${base}bridge/send`, { method: 'POST', body: JSON.stringify(body) }).then((r) => r.status);
    const data = { type: 'FeatureCollection', features: [] };
    expect(await post({ type: 'camera', center: [2, 48], zoom: 10 })).toBe(204);
    expect(
      await post([
        { type: 'addGeoJSON', id: 'a', data },
        { type: 'addGeoJSON', id: 'b', data },
        { type: 'basemap', name: 'none' },
        { type: 'remove', id: 'places.geojson' },
        { type: 'addGeoJSON', id: 'a', data, style: { color: 'red' } },
      ]),
    ).toBe(204);
    expect(await post({ nope: 1 })).toBe(400);
    expect((await fetch(`${base}bridge/send`, { method: 'POST', body: '{' })).status).toBe(400);
    const replay = (await events(`${base}bridge/events`, 6)) as Record<string, unknown>[];
    expect(replay.map((m) => m.type)).toEqual([
      'basemap',
      'camera',
      'addPointCloud',
      'addVectorTiles',
      'addGeoJSON',
      'addGeoJSON',
    ]);
    // `a` replaced at its place, with its new style
    expect(replay.slice(4).map((m) => m.id)).toEqual(['a', 'b']);
    expect(replay[4]?.style).toEqual({ color: 'red' });
  });

  test('live messages reach the viewers connected', async () => {
    const received = await events(`${base}bridge/events`, 7, () =>
      fetch(`${base}bridge/send`, { method: 'POST', body: JSON.stringify({ type: 'select', id: 'a', index: 0 }) }),
    );
    expect(received[6]).toEqual({ type: 'select', id: 'a', index: 0 });
  });

  test("the viewer's messages relayed to the hosts", async () => {
    const message = { type: 'click', lonLat: [2, 48], hit: null };
    const received = await events(`${base}bridge/host-events`, 1, () =>
      fetch(`${base}bridge/message`, { method: 'POST', body: JSON.stringify(message) }),
    );
    expect(received).toEqual([message]);
  });
});

describe('refusals', () => {
  test('another Host header', async () => {
    const port = new URL(base).port;
    const status = await new Promise<number>((resolve, reject) => {
      const req = httpRequest({ host: '127.0.0.1', port, path: '/', headers: { Host: `evil.com:${port}` } }, (res) => {
        res.resume();
        resolve(res.statusCode ?? 0);
      });
      req.on('error', reject);
      req.end();
    });
    expect(status).toBe(403);
  });

  test('a post from another origin', async () => {
    const body = JSON.stringify({ type: 'clear' });
    const response = await fetch(`${base}bridge/send`, {
      method: 'POST',
      body,
      headers: { Origin: 'https://evil.com' },
    });
    expect(response.status).toBe(403);
    const own = await fetch(`${base}bridge/message`, {
      method: 'POST',
      body: JSON.stringify({ type: 'ready', version: 'test' }),
      headers: { Origin: base.replace(/\/$/, '') },
    });
    expect(own.status).toBe(204);
  });
});
