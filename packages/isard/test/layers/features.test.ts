import { describe, expect, test } from 'bun:test';
import { interpolate, parseColor, rampTexels, stops } from '../../src/layers/features.ts';

describe('features', () => {
  test('parseColor: CSS colors, black when unknown or missing', () => {
    expect(parseColor('#fff')).toEqual([1, 1, 1, 1]);
    expect(parseColor('hsl(0, 100%, 50%)')).toEqual([1, 0, 0, 1]);
    expect(parseColor('nope')).toEqual([0, 0, 0, 1]);
    expect(parseColor(undefined as unknown as string)).toEqual([0, 0, 0, 1]);
  });

  test('stops: constants, up to four stops, more resampled', () => {
    expect(stops(3)).toEqual([0, 0, 0, 0, 3, 3, 3, 3]);
    expect(stops(Number.NaN)).toEqual([0, 0, 0, 0, 0, 0, 0, 0]);
    expect(
      stops([
        [10, 1],
        [14, 5],
      ]),
    ).toEqual([10, 14, 14, 14, 1, 5, 5, 5]);

    const many = stops([
      [0, 0],
      [1, 1],
      [2, 2],
      [3, 3],
      [6, 6],
    ]);

    expect(many.slice(0, 4)).toEqual([0, 2, 4, 6]);
    expect(many.slice(4)).toEqual([0, 2, 4, 6]);
  });

  test('interpolate: linear between stops, constant outside', () => {
    const s: [number, number][] = [
      [10, 2],
      [12, 6],
    ];

    expect(interpolate(s, 5)).toBe(2);
    expect(interpolate(s, 11)).toBe(4);
    expect(interpolate(s, 20)).toBe(6);
    expect(interpolate(7, 3)).toBe(7);
    expect(interpolate([], 3)).toBe(0);
  });

  test('rampTexels: the range and its ends', () => {
    const { rgba, min, max } = rampTexels([
      [10, '#000'],
      [0, '#fff'],
    ]);

    expect([min, max]).toEqual([0, 10]);
    expect([...rgba.subarray(0, 4)]).toEqual([255, 255, 255, 255]);
    expect([...rgba.subarray(255 * 4)]).toEqual([0, 0, 0, 255]);
  });
});
