import { describe, expect, test } from 'bun:test';
import { codes, type Glyph, Glyphs, shape } from '../../src/layers/glyphs.ts';
import type { Engine } from '../../src/wasm/isard.js';

const glyph = (x: number): Glyph => ({ width: 10, height: 12, left: 1, top: 10, advance: 12, x, y: 0 });
const glyphs = new Map<number, Glyph>([...'abc'].map((ch, i) => [ch.codePointAt(0) ?? 0, glyph(i * 20)]));
glyphs.set(32, { width: 0, height: 0, left: 0, top: 0, advance: 6 });
const options = {
  font: 'f',
  size: 24,
  maxWidth: 10,
  lineHeight: 1.2,
  anchor: 'center' as const,
  offset: [0, 0] as [number, number],
};

describe('glyphs', () => {
  test('codes', () => {
    expect([...codes([{ text: 'aba' }, { text: 'c' }])]).toEqual([97, 98, 99]);
  });

  test('shape: one quad per drawn glyph, centered on the anchor', () => {
    const { quads, box } = shape([{ text: 'ab c' }], () => glyphs, options);
    expect(quads.length).toBe(3);
    expect(box[0]).toBeCloseTo(-box[2], 6);
    expect(box[1]).toBeCloseTo(-box[3], 6);
  });

  test('shape: explicit and wrapped lines go down', () => {
    const one = shape([{ text: 'ab' }], () => glyphs, options);
    const two = shape([{ text: 'a\nb' }], () => glyphs, options);
    expect(two.box[3] - two.box[1]).toBeGreaterThan(one.box[3] - one.box[1]);
    const wrapped = shape([{ text: 'aaaa bbbb' }], () => glyphs, { ...options, maxWidth: 2 });
    expect(new Set(wrapped.quads.map((q) => q[1])).size).toBe(2);
  });

  test('shape: anchors and missing glyphs', () => {
    const left = shape([{ text: 'a' }], () => glyphs, { ...options, anchor: 'left' });
    expect(left.box[0]).toBeCloseTo(0, 6);
    expect(shape([{ text: 'zz' }], () => glyphs, options)).toEqual({
      quads: [],
      box: [0, 0, 0, 0],
      layout: [0, 0, 0, 0],
    });
  });

  test('shape: the layout box spans the advances and the line height, centered on the ink', () => {
    // Two glyphs of 12 px advance at 24 px: 24 px across; one line of 1.2 em: 28.8 px down
    const { box, layout } = shape([{ text: 'ab' }], () => glyphs, options);
    expect(layout[2] - layout[0]).toBeCloseTo(24, 6);
    expect(layout[3] - layout[1]).toBeCloseTo(28.8, 6);
    expect((layout[1] + layout[3]) / 2).toBeCloseTo((box[1] + box[3]) / 2, 6);
    expect((layout[0] + layout[2]) / 2).toBeCloseTo(0, 6);
    // Two lines: twice as tall; at half the size, half as large
    const two = shape([{ text: 'a\nb' }], () => glyphs, options);
    expect(two.layout[3] - two.layout[1]).toBeCloseTo(57.6, 6);
    const small = shape([{ text: 'ab' }], () => glyphs, { ...options, size: 12 });
    expect(small.layout[2] - small.layout[0]).toBeCloseTo(12, 6);
  });
});

/** A protobuf varint */
const varint = (n: number): number[] => {
  const out: number[] = [];
  let v = n;
  while (v >= 0x80) {
    out.push((v & 0x7f) | 0x80);
    v = Math.floor(v / 128);
  }
  out.push(v);
  return out;
};

/** A length-delimited field */
const field = (tag: number, bytes: number[]) => [...varint((tag << 3) | 2), ...varint(bytes.length), ...bytes];
const number = (tag: number, n: number) => [...varint(tag << 3), ...varint(n)];

/** A glyph range file: `count` glyphs of `side` × `side` px from code point `first` */
const rangeFile = (first: number, count: number, side: number): ArrayBuffer => {
  const glyphs = Array.from({ length: count }, (_, i) => {
    const bitmap = new Array<number>((side + 6) * (side + 6)).fill(i + 1);
    return field(
      3,
      [
        number(1, first + i),
        field(2, bitmap),
        number(3, side),
        number(4, side),
        number(5, 0),
        number(6, 0),
        number(7, side),
      ].flat(),
    );
  }).flat();
  return new Uint8Array(field(1, glyphs)).buffer;
};

describe('glyph atlas', () => {
  test('pages are added when the atlas is full, the glyphs placed keep their coordinates', async () => {
    const calls: { pages: number[]; updates: [number, number, number][] } = { pages: [], updates: [] };
    const engine = {
      setGlyphPages: (_w: number, _h: number, pages: number) => calls.pages.push(pages),
      updateGlyphPage: (page: number, _x: number, y: number, _w: number, h: number) => calls.updates.push([page, y, h]),
    };
    const original = globalThis.fetch;
    // 100 px glyphs (106 px bitmaps): 16 per 512 px page
    globalThis.fetch = (async () => new Response(rangeFile(0, 40, 100))) as unknown as typeof fetch;
    try {
      const atlas = new Glyphs('{fontstack}/{range}', 512);
      atlas.attach(engine as unknown as Engine);
      const glyphs = await atlas.load('f', [0]);
      expect(atlas.atlasSize).toBe(512);
      expect(atlas.pageCount).toBe(3);
      // The engine's pages doubled: 1, 2, 4
      expect(calls.pages).toEqual([1, 2, 4]);
      const placed = [...glyphs.values()];
      expect(placed.length).toBe(40);
      placed.forEach((g) => {
        expect(g.page).toBeDefined();
        // Rows of the pages before its own, then its place in it
        const local = (g.y ?? 0) - (g.page ?? 0) * 512;
        expect(local).toBeGreaterThanOrEqual(0);
        expect(local + 106).toBeLessThanOrEqual(512);
        expect((g.x ?? 0) + 106).toBeLessThanOrEqual(512);
      });
      expect(new Set(placed.map((g) => g.page))).toEqual(new Set([0, 1, 2]));
      // No two glyphs at the same place
      expect(new Set(placed.map((g) => `${g.x},${g.y}`)).size).toBe(40);
      // Every upload inside an allocated page
      calls.updates.forEach(([page, y, h]) => {
        expect(page).toBeLessThan(4);
        expect(y + h).toBeLessThanOrEqual(512);
      });
      // A glyph on the third page: its quad's atlas rows past the first two pages
      const last = glyphs.get(39);
      expect(last?.page).toBe(2);
      const { quads } = shape([{ text: String.fromCodePoint(39) }], () => glyphs, options);
      expect(quads[0]?.[5]).toBeGreaterThanOrEqual(1024);
    } finally {
      globalThis.fetch = original;
    }
  });

  test('the pages sent again (a new engine, more pages) are sent down to their last glyph only', async () => {
    const updates: [number, number, number, number][] = [];
    const engine = {
      setGlyphPages: () => undefined,
      updateGlyphPage: (page: number, _x: number, y: number, _w: number, h: number, data: Uint8Array) =>
        updates.push([page, y, h, data.length]),
    };
    const original = globalThis.fetch;
    // Three 100 px glyphs: one shelf of 106 rows
    globalThis.fetch = (async () => new Response(rangeFile(0, 3, 100))) as unknown as typeof fetch;
    try {
      const atlas = new Glyphs('{fontstack}/{range}', 512);
      atlas.attach(engine as unknown as Engine);
      // An empty atlas: nothing to send
      expect(updates).toEqual([]);
      await atlas.load('f', [0]);
      expect(updates).toEqual([[0, 0, 106, 106 * 512]]);
      updates.length = 0;
      // After a context loss
      atlas.attach(engine as unknown as Engine);
      expect(updates).toEqual([[0, 0, 106, 106 * 512]]);
    } finally {
      globalThis.fetch = original;
    }
  });
});
