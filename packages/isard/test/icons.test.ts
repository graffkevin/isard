import { describe, expect, test } from 'bun:test';
import {
  type Icon,
  Icons,
  layoutIcon,
  type Pixels,
  spriteFile,
  spriteName,
  spriteSources,
  stretch,
} from '../src/icons.ts';
import type { Engine } from '../src/wasm/isard.js';

/** An image of `w` × `h` pixels, each of one color */
function pixels(w: number, h: number, rgba: [number, number, number, number]): Pixels {
  const data = new Uint8Array(w * h * 4);
  for (let i = 0; i < w * h; i += 1) data.set(rgba, i * 4);
  return { width: w, height: h, data };
}

/** An engine recording the atlas calls */
function recorder() {
  const calls: [string, number[]][] = [];
  const engine = {
    setIconAtlas: (w: number, h: number) => calls.push(['set', [w, h]]),
    updateIconAtlas: (x: number, y: number, w: number, h: number, data: Uint8Array) =>
      calls.push(['update', [x, y, w, h, data.length]]),
  } as unknown as Engine;
  return { engine, calls };
}

const icon = (over: Partial<Icon> = {}): Icon => ({
  x: 100,
  y: 50,
  width: 20,
  height: 10,
  pixelRatio: 1,
  sdf: false,
  stretchX: undefined,
  stretchY: undefined,
  content: undefined,
  ...over,
});

const plain = {
  size: 1,
  anchor: 'center' as const,
  offset: [0, 0] as [number, number],
  rotate: 0,
  fit: 'none' as const,
  padding: [0, 0, 0, 0] as [number, number, number, number],
};

describe('sprites', () => {
  test("a style's sprite: one URL or several with their ids", () => {
    expect(spriteSources('https://s/sprite')).toEqual([{ id: 'default', url: 'https://s/sprite' }]);
    const many = [
      { id: 'default', url: 'https://s/a' },
      { id: 'shields', url: 'https://s/b' },
    ];
    expect(spriteSources(many)).toEqual(many);
    expect(spriteSources(undefined)).toEqual([]);
    expect(spriteSources('')).toEqual([]);
    expect(spriteSources([{ id: 'x' } as never, ...many])).toEqual(many);
    // Its images' names: their own in the default sprite, prefixed in the others
    expect(spriteName('default', 'bus')).toBe('bus');
    expect(spriteName('shields', 'road_2')).toBe('shields:road_2');
  });

  test('the files of a sprite: @2x on a high-density screen, before the query', () => {
    expect(spriteFile('https://s/sprite', 'json', 1)).toBe('https://s/sprite.json');
    expect(spriteFile('https://s/sprite', 'png', 2)).toBe('https://s/sprite@2x.png');
    expect(spriteFile('https://s/sprite?key=k', 'json', 1.5)).toBe('https://s/sprite@2x.json?key=k');
  });
});

describe('icon atlas', () => {
  test('icons packed on first use, premultiplied, only their rows uploaded', () => {
    const icons = new Icons();
    const { engine, calls } = recorder();
    icons.attach(engine);
    expect(calls).toEqual([['set', [256, 256]]]);
    icons.add('half', pixels(4, 2, [200, 100, 50, 128]), { pixelRatio: 2, sdf: true });
    expect(icons.has('half')).toBe(true);
    const half = icons.get('half');
    expect(half).toMatchObject({ width: 4, height: 2, pixelRatio: 2, sdf: true });
    // A texel of room around it
    expect([half?.x, half?.y]).toEqual([1, 1]);
    icons.flush();
    // Its rows (with the room around it), the whole width
    expect(calls[1]).toEqual(['update', [0, 0, 256, 4, 256 * 4 * 4]]);
    // The same icon again: nothing new to upload
    expect(icons.get('half')).toBe(half as Icon);
    icons.flush();
    expect(calls.length).toBe(2);
    expect(icons.get('none')).toBeUndefined();
  });

  test("a sprite's images read from the sheet once, on first use", () => {
    const icons = new Icons();
    let reads = 0;
    const sheet = pixels(8, 8, [255, 0, 0, 255]);
    icons.addSprite(
      'v0/',
      { a: { x: 0, y: 0, width: 4, height: 4 }, b: { x: 4, y: 4, width: 4, height: 4, pixelRatio: 2 } },
      () => {
        reads += 1;
        return sheet;
      },
    );
    expect(reads).toBe(0);
    const version = icons.version;
    expect(icons.get('v0/a')).toMatchObject({ width: 4, pixelRatio: 1 });
    expect(icons.get('v0/b')).toMatchObject({ width: 4, pixelRatio: 2 });
    expect(reads).toBe(1);
    // Off the sheet: none
    icons.addSprite('v1/', { c: { x: 6, y: 6, width: 4, height: 4 } }, () => sheet);
    expect(icons.get('v1/c')).toBeUndefined();
    // A sprite's images forgotten with it
    icons.remove('v0/', true);
    expect(icons.has('v0/a') || icons.has('v0/b')).toBe(false);
    expect(icons.version).toBeGreaterThan(version);
  });

  test('the atlas doubles when full, the icons keeping their places', () => {
    const icons = new Icons(1024);
    const { engine, calls } = recorder();
    icons.attach(engine);
    icons.add('a', pixels(200, 200, [0, 0, 0, 255]));
    const first = icons.get('a');
    expect(icons.atlasSize).toBe(256);
    icons.add('b', pixels(200, 200, [0, 0, 0, 255]));
    const second = icons.get('b');
    expect(icons.atlasSize).toBe(512);
    expect(icons.get('a')).toEqual(first as Icon);
    // Beside the first, on the wider shelf
    expect([second?.x, second?.y]).toEqual([203, 1]);
    // Allocated again, what it held sent again
    expect(calls.filter(([n]) => n === 'set').map(([, a]) => a)).toEqual([
      [256, 256],
      [512, 512],
    ]);
    // Past the texture limit: none, with a warning
    const warn = console.warn;
    console.warn = () => {};
    icons.add('huge', pixels(2000, 10, [0, 0, 0, 255]));
    expect(icons.get('huge')).toBeUndefined();
    console.warn = warn;
  });
});

describe('icon layout', () => {
  test('an icon around its anchor: centered, by its anchor, offset and scaled', () => {
    const a = layoutIcon(icon(), plain);
    expect(a.quads).toEqual([[-10, -5, 20, 10, 100, 50, 120, 60, -1, 0]]);
    expect(a.box).toEqual([-10, -5, 10, 5]);
    // Its bottom at the anchor, twice as large, shifted by 2 px (scaled with it)
    const b = layoutIcon(icon(), { ...plain, anchor: 'bottom', size: 2, offset: [2, 0] });
    expect(b.quads[0]?.slice(0, 4)).toEqual([-16, -20, 40, 20]);
    // An @2x image: half its pixels; a distance field
    const c = layoutIcon(icon({ pixelRatio: 2, sdf: true }), { ...plain, anchor: 'top-left' });
    expect(c.quads[0]?.slice(0, 4)).toEqual([0, 0, 10, 5]);
    expect(c.quads[0]?.[8]).toBe(-2);
  });

  test('a turned icon: its turn in the quad, its box the turned corners', () => {
    const { quads, box } = layoutIcon(icon(), { ...plain, rotate: 90 });
    expect(quads[0]?.[9]).toBeCloseTo(Math.PI / 2, 9);
    [-5, -10, 5, 10].forEach((want, i) => {
      expect(box[i]).toBeCloseTo(want, 9);
    });
  });

  test('an icon fitted to its text: across, down, both, with padding', () => {
    const text: [number, number, number, number] = [-30, -8, 30, 8];
    const padding: [number, number, number, number] = [1, 2, 3, 4];
    const both = layoutIcon(icon(), { ...plain, fit: 'both', padding, text });
    expect(both.quads).toEqual([[-34, -9, 66, 20, 100, 50, 120, 60, -1, 0]]);
    // Its width only: its own height (by its size), centered on the text
    const width = layoutIcon(icon(), { ...plain, size: 2, fit: 'width', padding, text });
    expect(width.quads[0]?.slice(0, 4)).toEqual([-34, -9, 66, 20]);
    const height = layoutIcon(icon(), { ...plain, fit: 'height', text });
    expect(height.quads[0]?.slice(0, 4)).toEqual([-10, -8, 20, 16]);
    // Without a text: laid out as usual
    expect(layoutIcon(icon(), { ...plain, fit: 'both' }).quads[0]?.slice(0, 4)).toEqual([-10, -5, 20, 10]);
  });

  test('a stretchable shield: its ends keep their size, its middle stretches, its content frames the text', () => {
    // 20 px across: the 4 px at each end fixed, the middle 12 stretching; the text between x = 3 and 17
    const pieces = stretch(20, [[4, 16]], [3, 17], 0, 60, 1);
    expect(pieces).toEqual([
      [0, 4, -3, 1],
      [4, 16, 1, 59],
      [16, 20, 59, 63],
    ]);
    // Two zones share the room by their lengths
    const two = stretch(
      20,
      [
        [2, 6],
        [14, 18],
      ],
      undefined,
      0,
      40,
      1,
    );
    expect(two.map(([, , a, b]) => b - a)).toEqual([2, 14, 8, 14, 2]);
    // Less room than the fixed parts: the stretchable parts vanish, nothing turns inside out
    const tight = stretch(20, [[4, 16]], undefined, 0, 4, 1);
    expect(tight.every(([, , a, b]) => b >= a)).toBe(true);
    // The quads of a stretched icon: one per piece, nine for both axes
    const shield = icon({ stretchX: [[4, 16]], stretchY: [[3, 7]], content: [3, 2, 17, 8] });
    const laid = layoutIcon(shield, { ...plain, fit: 'both', text: [-30, -8, 30, 8] });
    expect(laid.quads.length).toBe(9);
    expect(laid.quads[4]?.slice(4, 8)).toEqual([104, 53, 116, 57]);
    // The content box spans the text
    expect(laid.box[0]).toBeCloseTo(-33, 9);
    expect(laid.box[1]).toBeCloseTo(-10, 9);
  });
});
