/** Symbol layers: the labels sent to the engine, and those of groups kept between updates (a vector basemap's tiles) */
import { describe, expect, test } from 'bun:test';
import type { Feature } from '../src/features.ts';
import type { Glyph } from '../src/glyphs.ts';
import { SymbolLayer, symbolStyle } from '../src/symbols.ts';
import type { LabelCandidate } from '../src/vectorstyle.ts';
import { tileLabels } from '../src/vectortiles.ts';

/** Glyphs of every code point but those of `missing`, as a font file would give them */
function fakeGlyphs(missing = new Set<number>()) {
  const map = new Map<number, Glyph>();
  const fill = () => {
    for (let c = 32; c < 128; c += 1) {
      if (!missing.has(c))
        map.set(c, { width: 10, height: 14, left: 1, top: 12, advance: 12, x: c * 4, y: 0, page: 0 });
    }
  };
  fill();
  const loads: number[][] = [];
  return {
    version: 1,
    loads,
    load: async (_font: string, codes: Iterable<number>) => {
      loads.push([...codes]);
      return map;
    },
    /** The missing glyphs arrive (a range loaded) */
    arrive(this: { version: number }) {
      missing.clear();
      fill();
      this.version += 1;
    },
  };
}

/** A layer on a recording engine; `sent()` waits for the next labels sent */
function layer(glyphs = fakeGlyphs()) {
  let resolve: (args: unknown[]) => void = () => {};
  const engine = new Proxy(
    {},
    {
      get: (_, name) => {
        if (name === 'addSymbols') return () => 1;
        if (name === 'setSymbols') return (...args: unknown[]) => resolve(args);
        return () => undefined;
      },
    },
  );
  const host = { engine, glyphs, requestRender() {}, detach() {} };
  const symbols = new SymbolLayer<{ text: string }>(
    host as never,
    [],
    symbolStyle<{ text: string }>({ data: [], text: (p) => p.text }),
  );
  const next = () =>
    new Promise<[number, Float64Array, Float32Array, Float32Array, Float32Array]>((r) => {
      resolve = r as never;
    });
  symbols.attach(engine as never);
  return { symbols, glyphs, next };
}

const point = (text: string, lon: number): Feature<{ text: string }> => ({
  type: 'Feature',
  properties: { text },
  geometry: { type: 'Point', coordinates: [lon, 0] },
});

describe('symbol groups', () => {
  test('labels in groups are those of the same features as data, and groups seen before are not shaped again', async () => {
    const a = [point('ab', 1), point('c', 2)];
    const b = [point('de', 3)];
    const c = [point('fgh', 4)];
    const plain = layer();
    let sent = plain.next();
    plain.symbols.setData([...a, ...b, ...c]);
    const expected = await sent;
    const grouped = layer();
    sent = grouped.next();
    grouped.symbols.setGroups([a, b, c]);
    const got = await sent;
    for (let i = 1; i < 5; i += 1) expect([...(got[i] as Float32Array)]).toEqual([...(expected[i] as Float32Array)]);
    expect(grouped.symbols.features).toEqual([...a, ...b, ...c]);
    // Another order, a new group: only the new group's glyphs are asked for
    grouped.glyphs.loads.length = 0;
    const d = [point('i', 5)];
    sent = grouped.next();
    grouped.symbols.setGroups([c, d, a]);
    const moved = await sent;
    expect(grouped.glyphs.loads).toEqual([['i'.charCodeAt(0)]]);
    sent = plain.next();
    plain.symbols.setData([...c, ...d, ...a]);
    const again = await sent;
    // The quads' label indices follow the new order
    for (let i = 1; i < 5; i += 1) expect([...(moved[i] as Float32Array)]).toEqual([...(again[i] as Float32Array)]);
  });

  test('a new style shapes the groups again', async () => {
    const { symbols, glyphs, next } = layer();
    const a = [point('ab', 1)];
    let sent = next();
    symbols.setGroups([a]);
    await sent;
    glyphs.loads.length = 0;
    sent = next();
    symbols.setStyle({ size: 20 });
    await sent;
    expect(glyphs.loads.length).toBe(1);
  });

  test('a group shaped with glyphs missing is shaped again once glyphs arrive, not before', async () => {
    const glyphs = fakeGlyphs(new Set(['b'.charCodeAt(0)]));
    const { symbols, next } = layer(glyphs);
    const a = [point('ab', 1)];
    let sent = next();
    symbols.setGroups([a]);
    const first = await sent;
    // One glyph: one quad
    expect(first[4].length).toBe(10);
    glyphs.loads.length = 0;
    sent = next();
    symbols.setGroups([a]);
    await sent;
    expect(glyphs.loads.length).toBe(0);
    glyphs.arrive();
    sent = next();
    symbols.setGroups([a]);
    const second = await sent;
    expect(glyphs.loads.length).toBe(1);
    expect(second[4].length).toBe(20);
  });

  test('a style reading the index of the features shapes groups that moved again', async () => {
    const { symbols, glyphs, next } = layer();
    let sent = next();
    symbols.setStyle({ text: (p, i) => `${p.text}${i}` });
    await sent;
    const a = [point('a', 1)];
    const b = [point('b', 2)];
    sent = next();
    symbols.setGroups([a, b]);
    await sent;
    glyphs.loads.length = 0;
    sent = next();
    symbols.setGroups([b, a]);
    await sent;
    expect(glyphs.loads.length).toBe(1);
  });
});

describe('tileLabels', () => {
  const candidate = (layer: number, text: string, along?: [number, number]): LabelCandidate => ({
    layer,
    text,
    font: 'Noto Sans Regular',
    size: 12,
    color: '#000',
    haloColor: '#fff',
    haloWidth: 0,
    anchor: 'center',
    offset: [0, 0],
    maxWidth: 10,
    lineHeight: 1.2,
    overlap: false,
    lon: 2,
    lat: 48,
    ...(along ? { along } : {}),
  });

  test('features by style layer, the higher first, each in the order of the tile', () => {
    const groups = tileLabels([candidate(1, 'a'), candidate(3, 'b', [0.2, 0.1]), candidate(1, 'c')]);
    expect(groups.map(([layer, features]) => [layer, features.map((f) => f.properties.text)])).toEqual([
      [3, ['b']],
      [1, ['a', 'c']],
    ]);
    expect(groups[0]?.[1][0]?.geometry).toEqual({
      type: 'LineString',
      coordinates: [
        [1.9, 47.95],
        [2.1, 48.05],
      ],
    });
    expect(groups[1]?.[1][0]?.geometry).toEqual({ type: 'Point', coordinates: [2, 48] });
  });
});
