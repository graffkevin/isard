/** Symbol layers: the labels sent to the engine, and those of groups kept between updates (a vector basemap's tiles) */
import { describe, expect, test } from 'bun:test';
import type { LabelCandidate } from '../../src/basemap/vectorstyle.ts';
import { tileLabels } from '../../src/basemap/vectortiles.ts';
import type { Feature } from '../../src/layers/features.ts';
import type { Glyph } from '../../src/layers/glyphs.ts';
import { Icons } from '../../src/layers/icons.ts';
import { SymbolLayer, type SymbolOptions, symbolStyle } from '../../src/layers/symbols.ts';

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

type P = { text: string; icon?: string };

/** A layer on a recording engine; `sent()` waits for the next labels sent */
function layer(glyphs = fakeGlyphs(), style: Partial<SymbolOptions<P>> = {}, icons = new Icons()) {
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
  const host = { engine, glyphs, icons, requestRender() {}, detach() {} };
  const symbols = new SymbolLayer<P>(host as never, [], symbolStyle<P>({ data: [], text: (p) => p.text, ...style }));
  const next = () =>
    new Promise<[number, Float64Array, Float32Array, Float32Array, Float32Array]>((r) => {
      resolve = r as never;
    });
  symbols.attach(engine as never);
  return { symbols, glyphs, icons, next };
}

const point = (text: string, lon: number, icon?: string): Feature<P> => ({
  type: 'Feature',
  properties: { text, ...(icon ? { icon } : {}) },
  geometry: { type: 'Point', coordinates: [lon, 0] },
});

/** Floats per quad and per label style */
const QUAD = 11;
const LABEL = 36;
/** An image of `w` × `h` pixels */
const image = (w: number, h: number) => ({ width: w, height: h, data: new Uint8Array(w * h * 4).fill(255) });

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
    expect(first[4].length).toBe(QUAD);
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
    expect(second[4].length).toBe(2 * QUAD);
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

describe('icons', () => {
  test('a label with its icon: both flagged, the icon box and scale sent, the icon quads under the glyphs', async () => {
    const icons = new Icons();
    icons.add('poi', image(20, 10), { pixelRatio: 2 });
    const { symbols, next } = layer(
      fakeGlyphs(),
      {
        icon: (p) => p.icon ?? '',
        iconSize: [
          [10, 1],
          [14, 2],
        ],
      },
      icons,
    );
    const sent = next();
    symbols.setData([point('ab', 1, 'poi'), point('', 2, 'poi'), point('c', 3, 'missing')]);
    const [, anchors, boxes, styles, quads] = await sent;
    // Three labels: with its icon, the icon alone, the text alone (its image is missing)
    expect(anchors.length).toBe(6);
    expect(styles.length).toBe(3 * LABEL);
    const flags = [0, 1, 2].map((i) => styles[i * LABEL + 9]);
    // text 64, icon 128
    expect(flags).toEqual([192, 128, 64]);
    // The icon's box: 10 × 5 CSS pixels at its largest size (2), with 2 px of room
    expect([...styles.slice(20, 24)]).toEqual([-12, -7, 12, 7]);
    // Its scale: half its largest size at zoom 10
    expect([...styles.slice(28, 36)]).toEqual([10, 14, 14, 14, 0.5, 1, 1, 1]);
    // Its color: opaque black by default
    expect([...styles.slice(24, 28)]).toEqual([0, 0, 0, 1]);
    // The text box of the icon alone is empty
    expect([...boxes.slice(4, 8)]).toEqual([0, 0, 0, 0]);
    // Two icon quads first (scale -1), then the glyphs (three)
    const kinds = [];
    for (let q = 0; q < quads.length; q += QUAD) kinds.push(quads[q + 9]);
    expect(kinds.map((k) => (k ?? 0) < 0)).toEqual([true, true, false, false, false]);
    expect([quads[0], quads[QUAD]]).toEqual([0, 1]);
  });

  test('an image added later lays the labels missing it out again', async () => {
    const icons = new Icons();
    const { symbols, next } = layer(fakeGlyphs(), { icon: (p) => p.icon ?? '' }, icons);
    const a = [point('a', 1, 'pin')];
    let sent = next();
    symbols.setGroups([a]);
    expect((await sent)[3][9]).toBe(64);
    icons.add('pin', image(8, 8));
    sent = next();
    symbols.setGroups([a]);
    expect((await sent)[3][9]).toBe(192);
  });

  test('a shield stretched around its text, turning with it along its line; overlaps and optional parts', async () => {
    const icons = new Icons();
    icons.add('shield', image(10, 10));
    const line: Feature<P> = {
      type: 'Feature',
      properties: { text: 'A6', icon: 'shield' },
      geometry: {
        type: 'LineString',
        coordinates: [
          [0, 0],
          [1, 1],
        ],
      },
    };
    const { symbols, next } = layer(
      fakeGlyphs(),
      {
        icon: (p) => p.icon ?? '',
        placement: 'line',
        iconTextFit: 'both',
        iconTextFitPadding: [1, 2, 1, 2],
        overlap: 'allow',
        iconOverlap: 'ignore',
        iconOptional: true,
        size: 12,
      },
      icons,
    );
    const sent = next();
    symbols.setData([line]);
    const [, , boxes, styles, quads] = await sent;
    // text, icon, text allowed to overlap 1, icon ignored 8, icon optional 32, along its line 256 and 512, upright 1024
    expect(styles[9]).toBe(64 | 128 | 1 | 8 | 32 | 256 | 512 | 1024);
    // A direction
    expect(styles[10] !== 0 || styles[11] !== 0).toBe(true);
    // The icon frames the text (its layout box and the padding): as wide as two advances of 6 px and 4 px of padding
    expect(quads[3]).toBeCloseTo(12 + 4, 6);
    expect(quads[4]).toBeCloseTo(1.2 * 12 + 2, 6);
    // Scaled as the text
    expect([...styles.slice(28, 36)]).toEqual([...styles.slice(12, 20)]);
    expect(boxes.length).toBe(4);
  });

  test('an icon turning with the map at a point takes the east as its direction', async () => {
    const icons = new Icons();
    icons.add('arrow', image(4, 4));
    const { symbols, next } = layer(
      fakeGlyphs(),
      { icon: 'arrow', iconRotationAlignment: 'map', iconRotate: 90 },
      icons,
    );
    const sent = next();
    symbols.setData([point('', 1)]);
    const [, , , styles, quads] = await sent;
    expect(styles[9]).toBe(128 | 512);
    expect([styles[10], styles[11]]).toEqual([1, 0]);
    expect(quads[10]).toBeCloseTo(Math.PI / 2, 6);
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
