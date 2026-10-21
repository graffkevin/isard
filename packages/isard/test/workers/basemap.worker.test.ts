/** The vector basemap worker, driven through its messages: a recording Canvas 2D, vector tiles written by hand */
import { afterAll, beforeEach, describe, expect, mock, test } from 'bun:test';
import type { StyleLayer } from '../../src/basemap/vectorstyle.ts';
import type { BasemapRequest, BasemapResponse, BasemapWork } from '../../src/workers/basemap.worker.ts';
import { setGlobals } from '../fakes.ts';

// ── A vector tile writer (protobuf) ──

const varint = (value: number): number[] => {
  let v = value;
  const out: number[] = [];
  do {
    const b = v & 0x7f;
    v = Math.floor(v / 128);
    out.push(v > 0 ? b | 0x80 : b);
  } while (v > 0);
  return out;
};
const zigzag = (n: number) => (n < 0 ? -2 * n - 1 : 2 * n);
const tag = (field: number, wire: number) => varint((field << 3) | wire);
const bytes = (field: number, data: number[]) => [...tag(field, 2), ...varint(data.length), ...data];
const string = (field: number, s: string) => bytes(field, [...new TextEncoder().encode(s)]);
const uint = (field: number, v: number) => [...tag(field, 0), ...varint(v)];
const command = (id: number, count: number) => varint((count << 3) | id);

/** A layer of one feature: a point (type 1) or a line (type 2) through `points` (tile units), `name` its property */
const layer = (name: string, type: 1 | 2, points: [number, number][], text?: string): number[] => {
  const geometry: number[] = [];
  let [x, y] = [0, 0];
  points.forEach(([px, py], i) => {
    if (i === 0) geometry.push(...command(1, 1));
    else if (i === 1) geometry.push(...command(2, points.length - 1));
    geometry.push(...varint(zigzag(px - x)), ...varint(zigzag(py - y)));
    [x, y] = [px, py];
  });
  const feature = [...(text ? bytes(2, [0, 0]) : []), ...uint(3, type), ...bytes(4, geometry)];
  const properties = text ? [...string(3, 'name'), ...bytes(4, string(1, text))] : [];
  return bytes(3, [...string(1, name), ...properties, ...bytes(2, feature), ...uint(5, 4096)]);
};

// ── A recording canvas ──

type Op = [name: string, value: unknown[]];

class FakeCanvas {
  static all: FakeCanvas[] = [];
  ops: Op[] = [];
  transferred = 0;
  constructor(
    readonly width: number,
    readonly height: number,
  ) {
    FakeCanvas.all.push(this);
  }
  getContext() {
    const ops = this.ops;
    return new Proxy(
      {},
      {
        get: (_, name: string) => {
          if (name === 'getImageData') return () => ({ data: new Uint8ClampedArray(4), imageData: true });
          if (name === 'createPattern') return () => ({ setTransform() {} });
          return (...args: unknown[]) => ops.push([name, args]);
        },
        set: (_, name: string, value: unknown) => {
          ops.push([`=${name}`, [value]]);
          return true;
        },
      },
    );
  }
  transferToImageBitmap() {
    this.transferred += 1;
    return { transferred: true, close() {} };
  }
}

const tiles = new Map<string, Uint8Array<ArrayBuffer> | number>();
const posted: BasemapResponse[] = [];
const bitmaps: { source: unknown; options: unknown }[] = [];
const restore = setGlobals({
  OffscreenCanvas: FakeCanvas,
  fetch: mock(async (url: string) => {
    const body = tiles.get(url);
    if (body === undefined || typeof body === 'number')
      return new Response(null, { status: typeof body === 'number' ? body : 404 });
    return new Response(body);
  }),
  createImageBitmap: async (source: unknown, options?: unknown) => {
    bitmaps.push({ source, options });
    return { width: 256, height: 256, close() {} };
  },
  postMessage: (message: BasemapResponse) => posted.push(message),
  ImageData: class {
    constructor(
      readonly data: Uint8ClampedArray,
      readonly width: number,
      readonly height: number,
    ) {}
  },
});
afterAll(restore);

const worker = await import('../../src/workers/basemap.worker.ts');
const onmessage = (globalThis as unknown as { onmessage: (e: { data: BasemapRequest }) => Promise<void> }).onmessage;

beforeEach(() => {
  posted.length = 0;
  bitmaps.length = 0;
  FakeCanvas.all = [];
});

let keys = 0;
/** A renderer of these layers, its key */
const renderer = async (layers: StyleLayer[], rasters = {}): Promise<string> => {
  const key = `k${keys++}`;
  await onmessage({
    data: { type: 'init', key, tiles: 'https://t/{z}/{x}/{y}.pbf', maxzoom: 14, layers, rasters, workers: 2 },
  });
  return key;
};

let ids = 0;
const ask = async (work: BasemapWork): Promise<BasemapResponse> => {
  const id = ++ids;
  await onmessage({ data: { ...work, id } as BasemapRequest });
  const answer = posted.find((m) => m.id === id);
  if (!answer) throw new Error('no answer');
  return answer;
};

/** The values a canvas property was given, in order */
const sets = (canvas: FakeCanvas | undefined, name: string) =>
  (canvas?.ops ?? []).filter(([n]) => n === `=${name}`).map(([, [v]]) => v);

describe('render', () => {
  test('a line with a gap is one wide line (casing); a tile without background is read back unpremultiplied', async () => {
    tiles.set(
      'https://t/4/1/1.pbf',
      Uint8Array.from(
        layer('roads', 2, [
          [0, 0],
          [100, 100],
        ]),
      ),
    );
    const key = await renderer([
      {
        id: 'road',
        type: 'line',
        source: 'osm',
        'source-layer': 'roads',
        paint: { 'line-width': 2, 'line-gap-width': 4, 'line-color': '#f00' },
      },
    ]);
    const answer = await ask({ type: 'render', key, z: 5, x: 2, y: 2, size: 512 });
    expect('image' in answer).toBe(true);
    const canvas = FakeCanvas.all[0];
    // (gap + 2 × width) CSS pixels, 2 canvas pixels each at 512 px
    expect(sets(canvas, 'lineWidth')).toEqual([16]);
    expect(canvas?.ops.some(([n]) => n === 'stroke')).toBe(true);
    expect(canvas?.transferred).toBe(0);
    expect(bitmaps.at(-1)).toEqual({
      source: expect.objectContaining({ imageData: true }),
      options: { premultiplyAlpha: 'none' },
    });
  });

  test('an opaque background: the canvas is moved out as it is, and reused by the next tile', async () => {
    tiles.set(
      'https://t/4/3/3.pbf',
      Uint8Array.from(
        layer('roads', 2, [
          [0, 0],
          [100, 100],
        ]),
      ),
    );
    const key = await renderer([{ id: 'bg', type: 'background', paint: { 'background-color': '#fff' } }]);
    await ask({ type: 'render', key, z: 5, x: 6, y: 6, size: 1024 });
    expect(FakeCanvas.all.length).toBe(1);
    expect(FakeCanvas.all[0]?.transferred).toBe(1);
    expect(bitmaps).toEqual([]);
    await ask({ type: 'render', key, z: 5, x: 7, y: 6, size: 1024 });
    expect(FakeCanvas.all.length).toBe(1);
    expect(FakeCanvas.all[0]?.transferred).toBe(2);
    // A background of partial opacity is no opaque tile
    const faint = await renderer([
      { id: 'bg', type: 'background', paint: { 'background-color': '#fff', 'background-opacity': 0.5 } },
    ]);
    await ask({ type: 'render', key: faint, z: 5, x: 6, y: 7, size: 1024 });
    expect(FakeCanvas.all[0]?.transferred).toBe(2);
    expect(bitmaps.length).toBe(1);
  });

  test('a missing vector tile with nothing else drawn: no image', async () => {
    tiles.set('https://t/4/5/5.pbf', 404);
    const key = await renderer([{ id: 'road', type: 'line', source: 'osm', 'source-layer': 'roads' }]);
    expect(await ask({ type: 'render', key, z: 5, x: 10, y: 10, size: 256 })).toEqual({ id: ids, missing: true });
  });

  test("a style's raster layer is drawn under the others, with its opacity, from the source tile covering it", async () => {
    tiles.set('https://t/4/6/6.pbf', 404);
    tiles.set('https://r/5/12/12.png', Uint8Array.of(1));
    const key = await renderer([{ id: 'relief', type: 'raster', source: 'relief', paint: { 'raster-opacity': 0.5 } }], {
      relief: { tiles: 'https://r/{z}/{x}/{y}.png', maxzoom: 10, tileSize: 256 },
    });
    const answer = await ask({ type: 'render', key, z: 5, x: 12, y: 12, size: 128 });
    expect('image' in answer).toBe(true);
    const canvas = FakeCanvas.all[0];
    expect(sets(canvas, 'globalAlpha')).toEqual([1, 0.5, 1]);
    const draw = canvas?.ops.find(([n]) => n === 'drawImage');
    expect(draw?.[1].slice(1)).toEqual([0, 0, 256, 256, 0, 0, 128, 128]);
  });
});

describe('labels', () => {
  test('a text over its icon (a road shield) is listed with its icon; other labels without', async () => {
    tiles.set(
      'https://t/6/1/1.pbf',
      Uint8Array.from([...layer('places', 1, [[100, 200]], 'Paris'), ...layer('shields', 1, [[300, 300]], 'A6')]),
    );
    const key = await renderer([
      {
        id: 'place',
        type: 'symbol',
        source: 'osm',
        'source-layer': 'places',
        layout: { 'text-field': '{name}' },
        paint: { 'text-halo-width': 1 },
      },
      {
        id: 'shield',
        type: 'symbol',
        source: 'osm',
        'source-layer': 'shields',
        layout: {
          'text-field': '{name}',
          'icon-image': 'road_{name}',
          'icon-size': ['interpolate', ['linear'], ['zoom'], 5, 1, 7, 2],
          'icon-text-fit': 'both',
          'icon-text-fit-padding': [2, 4],
          'icon-allow-overlap': true,
          'text-optional': true,
        },
        paint: { 'icon-opacity': 0.5 },
      },
    ]);
    const answer = await ask({ type: 'labels', key, z: 6, x: 1, y: 1 });
    const labels = 'labels' in answer ? answer.labels : [];
    expect(labels.map((l) => l.text)).toEqual(['Paris', 'A6']);
    expect(labels[0]?.haloWidth).toBe(1);
    expect(labels[0]?.icon).toBeUndefined();
    expect(labels[1]?.textOptional).toBe(true);
    expect(labels[1]?.icon).toEqual({
      image: 'road_A6',
      // Zoom stops for the engine
      size: [
        [5, 1],
        [6, 1.5],
        [7, 2],
      ],
      anchor: 'center',
      offset: [0, 0],
      rotate: 0,
      alignment: 'viewport',
      overlap: 'allow',
      optional: false,
      fit: 'both',
      padding: [2, 4, 2, 4],
      color: '#000',
      opacity: 0.5,
    });
  });

  test('icons alone along lines every symbol-spacing, with the direction of the line; placements by zoom', async () => {
    // A line across the tile, left to right, 4096 units long: a tile is 512 CSS pixels across
    tiles.set(
      'https://t/6/2/2.pbf',
      Uint8Array.from([
        ...layer('roads', 2, [
          [0, 2048],
          [4095, 2048],
        ]),
        ...layer(
          'names',
          2,
          [
            [0, 1000],
            [4000, 1000],
          ],
          'N7',
        ),
      ]),
    );
    const key = await renderer([
      {
        id: 'oneway',
        type: 'symbol',
        source: 'osm',
        'source-layer': 'roads',
        layout: { 'icon-image': 'arrow', 'symbol-placement': 'line', 'symbol-spacing': 128 },
      },
      {
        id: 'number',
        type: 'symbol',
        source: 'osm',
        'source-layer': 'names',
        layout: {
          'text-field': '{name}',
          'icon-image': 'shield',
          'symbol-placement': ['step', ['zoom'], 'point', 6, 'line'],
          'text-rotation-alignment': 'viewport',
          'icon-rotation-alignment': 'viewport',
        },
      },
    ]);
    const answer = await ask({ type: 'labels', key, z: 6, x: 2, y: 2 });
    const labels = 'labels' in answer ? answer.labels : [];
    const arrows = labels.filter((l) => l.icon?.image === 'arrow');
    // Every 128 px of a 512 px line, the first 64 px in
    expect(arrows.length).toBe(4);
    expect(arrows.every((a) => a.text === '' && a.icon?.alignment === 'map' && (a.along?.[0] ?? 0) > 0)).toBe(true);
    const [l0 = 0, l1 = 0, l2 = 0] = arrows.map((a) => a.lon);
    expect(l1 - l0).toBeCloseTo(l2 - l1, 9);
    // The number at the middle of its line, horizontal: a direction for its icon only
    const number = labels.find((l) => l.text === 'N7');
    expect(number?.along).toBeDefined();
    expect(number?.textAlong).toBeUndefined();
    expect(number?.icon?.alignment).toBe('viewport');
  });

  test('lineAnchors: half a spacing in, then every spacing; the middle of a shorter line', () => {
    const anchors = worker.lineAnchors([0, 0, 100, 0, 100, 100], 50);
    expect(anchors.map(([x, y]) => [x, y])).toEqual([
      [25, 0],
      [75, 0],
      [100, 25],
      [100, 75],
    ]);
    expect(anchors[2]?.slice(2)).toEqual([100, 0, 100, 100]);
    expect(worker.lineAnchors([0, 0, 10, 0], 50).map(([x]) => x)).toEqual([5]);
    expect(worker.lineAnchors([0, 0], 50)).toEqual([]);
  });
});

describe('cache', () => {
  test("each worker keeps its share of the pool's decoded tiles", () => {
    expect(worker.cacheShare(1)).toBe(96);
    expect(worker.cacheShare(4)).toBe(24);
    expect(worker.cacheShare(16)).toBe(16);
  });
});

describe("the map's other layers", () => {
  test('a relief tile decoded into altitudes, moved out; a missing one answered as such', async () => {
    tiles.set('https://dem/3/4/5.png', Uint8Array.of(1));
    const answer = await ask({ type: 'heights', url: 'https://dem/3/4/5.png', encoding: 'terrarium', floor: 0 });
    // The colors as stored
    expect(bitmaps[0]?.options).toEqual({ premultiplyAlpha: 'none', colorSpaceConversion: 'none' });
    // One transparent black pixel of the fake canvas: -32768 m, raised to the floor
    expect('heights' in answer && [...answer.heights]).toEqual([0]);
    tiles.set('https://dem/3/4/6.png', 404);
    expect(await ask({ type: 'heights', url: 'https://dem/3/4/6.png', encoding: 'mapbox' })).toMatchObject({
      missing: true,
    });
  });

  test('a tile reprojected from the images of its plan, kept for the next tiles; straight alpha', async () => {
    tiles.set('https://eq/0/0/0.jpg', Uint8Array.of(1));
    tiles.set('https://eq/0/1/0.jpg', Uint8Array.of(1));
    // The world in two source tiles side by side, onto a tile of 4 pixels
    const sources = [
      { url: 'https://eq/0/0/0.jpg', x: 0, y: 0, w: 256, h: 256 },
      { url: 'https://eq/0/1/0.jpg', x: 256, y: 0, w: 256, h: 256 },
    ];
    const job = {
      size: 4,
      width: 512,
      height: 256,
      sources,
      n: 1,
      mesh: Float32Array.of(0, 0, 512, 0, 0, 256, 512, 256),
    };
    const answer = await ask({ type: 'reproject', job, cached: 16 });
    expect(answer).toMatchObject({ image: { width: 256 } });
    // Both drawn into the mosaic, read back, the tile made of its pixels as they are
    const canvas = FakeCanvas.all.at(-1);
    expect(canvas?.ops.filter(([name]) => name === 'drawImage').map(([, [, ...rect]]) => rect)).toEqual([
      [0, 0, 256, 256],
      [256, 0, 256, 256],
    ]);
    expect(bitmaps.at(-1)?.options).toEqual({ premultiplyAlpha: 'none' });
    const fetched = bitmaps.length;
    await ask({ type: 'reproject', job, cached: 16 });
    expect(bitmaps.length).toBe(fetched + 1);
    // Nothing at all there: no image
    const none = { ...job, sources: [{ ...sources[0], url: 'https://none/0/0/0.jpg' }] };
    expect(await ask({ type: 'reproject', job: none as typeof job, cached: 16 })).toMatchObject({ missing: true });
  });

  test('an error is answered as one (the engine retries)', async () => {
    tiles.set('https://dem/2/1/1.png', 500);
    const answer = await ask({ type: 'heights', url: 'https://dem/2/1/1.png', encoding: 'terrarium' });
    expect('error' in answer && answer.error).toContain('500');
  });
});
