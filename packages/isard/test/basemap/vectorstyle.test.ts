import { describe, expect, spyOn, test } from 'bun:test';
import {
  type Context,
  color,
  evaluate,
  filter,
  isExpression,
  type Json,
  labelFont,
  text,
  zoomStops,
} from '../../src/basemap/vectorstyle.ts';

const ctx: Context = {
  zoom: 12,
  type: 'LineString',
  properties: { name: 'Seine', class: 'river', width: 4, rank: 2, empty: '' },
};

describe('evaluate', () => {
  test('constants, legacy functions, lookups', () => {
    expect(evaluate(3, ctx)).toBe(3);
    expect(
      evaluate(
        {
          stops: [
            [10, 1],
            [14, 5],
          ],
        },
        ctx,
      ),
    ).toBe(3);
    expect(evaluate({ property: 'class', type: 'categorical', stops: [['river', 'blue']] }, ctx)).toBe('blue');
    expect(evaluate(['get', 'name'], ctx)).toBe('Seine');
    expect(evaluate(['has', 'nope'], ctx)).toBe(false);
    expect(evaluate(['zoom'], ctx)).toBe(12);
    expect(evaluate(['geometry-type'], ctx)).toBe('LineString');
    expect(evaluate(['literal', ['a', 'b']], ctx)).toEqual(['a', 'b']);
  });

  test('arithmetic', () => {
    expect(evaluate(['+', ['get', 'width'], 1, 2], ctx)).toBe(7);
    expect(evaluate(['-', 10, ['get', 'width']], ctx)).toBe(6);
    expect(evaluate(['-', 3], ctx)).toBe(-3);
    expect(evaluate(['*', ['get', 'width'], ['zoom']], ctx)).toBe(48);
    expect(evaluate(['/', 9, 3], ctx)).toBe(3);
    expect(evaluate(['%', 9, 4], ctx)).toBe(1);
    expect(evaluate(['^', 2, 10], ctx)).toBe(1024);
    expect(evaluate(['min', 3, 1, 2], ctx)).toBe(1);
    expect(evaluate(['max', 3, 1, 2], ctx)).toBe(3);
    expect(evaluate(['abs', -2], ctx)).toBe(2);
    expect(evaluate(['round', -2.5], ctx)).toBe(-3);
    expect(evaluate(['floor', 2.7], ctx)).toBe(2);
    expect(evaluate(['ceil', 2.1], ctx)).toBe(3);
    expect(evaluate(['sqrt', 16], ctx)).toBe(4);
  });

  test('strings and conversions', () => {
    expect(evaluate(['concat', ['get', 'name'], ' ', ['get', 'rank']], ctx)).toBe('Seine 2');
    expect(evaluate(['upcase', ['get', 'name']], ctx)).toBe('SEINE');
    expect(evaluate(['downcase', 'ABC'], ctx)).toBe('abc');
    expect(evaluate(['to-string', ['get', 'width']], ctx)).toBe('4');
    expect(evaluate(['to-number', '12.5'], ctx)).toBe(12.5);
    expect(evaluate(['to-number', 'x', '', 7], ctx)).toBe(7);
    expect(evaluate(['to-boolean', ['get', 'empty']], ctx)).toBe(false);
    expect(evaluate(['to-boolean', ['get', 'name']], ctx)).toBe(true);
    expect(evaluate(['coalesce', ['get', 'nope'], ['get', 'name']], ctx)).toBe('Seine');
    expect(
      evaluate(['format', ['get', 'name'], {}, ' (', {}, ['get', 'class'], { 'font-scale': 0.8 }, ')', {}], ctx),
    ).toBe('Seine (river)');
  });

  test('decisions and interpolation', () => {
    expect(evaluate(['case', ['==', ['get', 'class'], 'road'], 1, ['>', ['get', 'width'], 3], 2, 3], ctx)).toBe(2);
    expect(evaluate(['match', ['get', 'class'], ['lake', 'river'], 'water', 'other'], ctx)).toBe('water');
    expect(evaluate(['step', ['zoom'], 0, 10, 1, 13, 2], ctx)).toBe(1);
    expect(evaluate(['interpolate', ['linear'], ['zoom'], 10, 0, 14, 8], ctx)).toBe(4);
    expect(evaluate(['interpolate', ['linear'], ['zoom'], 10, '#000000', 14, '#ffffff'], ctx)).toBe(
      'rgba(128,128,128,1)',
    );
    expect(evaluate(['in', 'ei', ['get', 'name']], ctx)).toBe(true);
    expect(evaluate(['in', 'river', ['literal', ['lake', 'river']]], ctx)).toBe(true);
  });

  test('an unsupported operator gives null, with one warning', () => {
    const warn = spyOn(console, 'warn').mockImplementation(() => undefined);

    expect(evaluate(['distance', ['literal', {}]], ctx)).toBeNull();
    expect(evaluate(['distance', ['literal', {}]], ctx)).toBeNull();
    expect(warn).toHaveBeenCalledTimes(1);
    warn.mockRestore();
  });

  test('literal arrays are not expressions', () => {
    expect(isExpression(['Source Sans Pro Regular'])).toBe(false);
    expect(isExpression(['get', 'name'])).toBe(true);
    expect(isExpression([1, 2])).toBe(false);
  });
});

describe('filter', () => {
  test('legacy filters', () => {
    expect(filter(['==', 'class', 'river'], ctx)).toBe(true);
    expect(filter(['!=', 'class', 'river'], ctx)).toBe(false);
    expect(filter(['in', 'class', 'lake', 'river'], ctx)).toBe(true);
    expect(filter(['!in', 'class', 'lake', 'river'], ctx)).toBe(false);
    expect(filter(['has', 'name'], ctx)).toBe(true);
    expect(filter(['!has', 'name'], ctx)).toBe(false);
    expect(filter(['==', '$type', 'LineString'], ctx)).toBe(true);
    expect(filter(['all', ['>=', 'width', 4], ['<', 'rank', 3]], ctx)).toBe(true);
    expect(filter(['none', ['==', 'class', 'road']], ctx)).toBe(true);
  });

  test('expression filters', () => {
    expect(filter(['==', ['get', 'class'], 'river'], ctx)).toBe(true);
    expect(filter(['all', ['>', ['get', 'width'], 3], ['!', ['has', 'nope']]], ctx)).toBe(true);
    expect(filter(['in', ['get', 'class'], ['literal', ['road', 'path']]], ctx)).toBe(false);
    expect(filter(true, ctx)).toBe(true);
    expect(filter(undefined, ctx)).toBe(true);
  });

  test('legacy and expression parts mixed', () => {
    const mixed: Json = ['all', ['==', 'class', 'river'], ['>=', ['zoom'], 10]];

    expect(filter(mixed, ctx)).toBe(true);
    expect(filter(mixed, { ...ctx, zoom: 8 })).toBe(false);
    expect(filter(['any', ['==', 'class', 'road'], ['==', ['get', 'rank'], 2]], ctx)).toBe(true);
  });

  test('an unsupported operator drops the feature', () => {
    const warn = spyOn(console, 'warn').mockImplementation(() => undefined);

    expect(filter(['within', ['literal', {}]], ctx)).toBe(false);
    warn.mockRestore();
  });
});

describe('text and color', () => {
  test('{token} fields and expressions', () => {
    expect(text('{name} ({class})', ctx)).toBe('Seine (river)');
    expect(text('{nope}', ctx)).toBe('');
    expect(text(['format', ['get', 'name'], {}], ctx)).toBe('Seine');
    expect(text(['get', 'rank'], ctx)).toBe('2');
    expect(text(undefined, ctx)).toBe('');
  });

  test('colors', () => {
    expect(color('#ff0000')).toEqual([1, 0, 0, 1]);
    expect(color('navy')?.[2]).toBeCloseTo(0x80 / 255, 6);
    expect(color(['get', 'x'])).toBeUndefined();
  });
});

describe('zoomStops', () => {
  test('a zoom interpolation as stops: its own, and the whole zooms between', () => {
    expect(zoomStops(['interpolate', ['linear'], ['zoom'], 8, 9, 12, 10])).toEqual([
      [8, 9],
      [9, 9.25],
      [10, 9.5],
      [11, 9.75],
      [12, 10],
    ]);

    // Exponential: followed at the whole zooms
    const curve = zoomStops(['interpolate', ['exponential', 2], ['zoom'], 0, 0, 2, 3]);

    expect(curve.map(([z]) => z)).toEqual([0, 1, 2]);
    expect(curve[1]?.[1]).toBeCloseTo(1, 6);
  });

  test('a step kept sharp; a constant, one stop', () => {
    expect(zoomStops(['step', ['zoom'], 10, 12, 14])).toEqual([
      [11.99, 10],
      [12, 14],
    ]);
    expect(zoomStops(['+', 1, 2])).toEqual([[0, 3]]);
  });

  test('fonts without a weight take the regular one', () => {
    expect(labelFont('Noto Sans')).toBe('Noto Sans Regular');
    expect(labelFont('Noto Sans Italic')).toBe('Noto Sans Italic');
  });
});
