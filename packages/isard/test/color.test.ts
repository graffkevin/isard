import { describe, expect, test } from 'bun:test';
import { parseCssColor } from '../src/color.ts';

const close = (actual: number[] | undefined, expected: number[]) => {
  expect(actual).toBeDefined();
  for (const [i, v] of expected.entries()) expect(actual?.[i]).toBeCloseTo(v, 3);
};

describe('parseCssColor', () => {
  test('hex forms', () => {
    close(parseCssColor('#fff'), [1, 1, 1, 1]);
    close(parseCssColor('#f008'), [1, 0, 0, 0x88 / 255]);
    close(parseCssColor('#336699'), [0x33 / 255, 0x66 / 255, 0x99 / 255, 1]);
    close(parseCssColor('#33669980'), [0x33 / 255, 0x66 / 255, 0x99 / 255, 0x80 / 255]);
    expect(parseCssColor('#12')).toBeUndefined();
    expect(parseCssColor('#ggg')).toBeUndefined();
  });

  test('rgb() and rgba(), comma and space syntax', () => {
    close(parseCssColor('rgb(255, 0, 51)'), [1, 0, 0.2, 1]);
    close(parseCssColor('rgba(255,0,51,0.5)'), [1, 0, 0.2, 0.5]);
    close(parseCssColor('rgb(255 0 51 / 50%)'), [1, 0, 0.2, 0.5]);
    close(parseCssColor('rgb(100% 0% 50%)'), [1, 0, 0.5, 1]);
  });

  test('hsl() and hsla()', () => {
    close(parseCssColor('hsl(0, 100%, 50%)'), [1, 0, 0, 1]);
    close(parseCssColor('hsl(120 100% 25%)'), [0, 0.5, 0, 1]);
    close(parseCssColor('hsla(240, 100%, 50%, 0.25)'), [0, 0, 1, 0.25]);
    close(parseCssColor('hsl(360deg 0% 100%)'), [1, 1, 1, 1]);
  });

  test('names and transparent', () => {
    close(parseCssColor('rebeccapurple'), [0x66 / 255, 0x33 / 255, 0x99 / 255, 1]);
    close(parseCssColor(' White '), [1, 1, 1, 1]);
    close(parseCssColor('lightgoldenrodyellow'), [0xfa / 255, 0xfa / 255, 0xd2 / 255, 1]);
    close(parseCssColor('transparent'), [0, 0, 0, 0]);
  });

  test('not colors', () => {
    expect(parseCssColor('nope')).toBeUndefined();
    expect(parseCssColor(undefined)).toBeUndefined();
    expect(parseCssColor(3)).toBeUndefined();
    expect(parseCssColor('rgb(a, b, c)')).toBeUndefined();
  });
});
