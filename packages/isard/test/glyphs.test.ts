import { describe, expect, test } from 'bun:test';
import { codes, type Glyph, shape } from '../src/glyphs.ts';

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
    expect(shape([{ text: 'zz' }], () => glyphs, options)).toEqual({ quads: [], box: [0, 0, 0, 0] });
  });
});
