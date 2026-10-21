import { describe, expect, test } from 'bun:test';
import { decode } from '../../src/basemap/mvt.ts';

// ── A small protobuf writer ──

/** A varint; a bigint for 64-bit values (negative ones in two's complement, 10 bytes) */
function varint(value: number | bigint): number[] {
  let v = BigInt.asUintN(64, BigInt(value));
  const out: number[] = [];
  do {
    const b = Number(v & 0x7fn);
    v >>= 7n;
    out.push(v > 0n ? b | 0x80 : b);
  } while (v > 0n);
  return out;
}
const zigzag = (n: number) => (n < 0 ? -2 * n - 1 : 2 * n);
const tag = (field: number, wire: number) => varint((field << 3) | wire);
const bytes = (field: number, data: number[]) => [...tag(field, 2), ...varint(data.length), ...data];
const string = (field: number, s: string) => bytes(field, [...new TextEncoder().encode(s)]);
const uint = (field: number, v: number | bigint) => [...tag(field, 0), ...varint(v)];
const double = (field: number, v: number) => {
  const b = new Uint8Array(8);
  new DataView(b.buffer).setFloat64(0, v, true);
  return [...tag(field, 1), ...b];
};
const command = (id: number, count: number) => varint((count << 3) | id);

function tile(layer: number[]): Uint8Array {
  return Uint8Array.from(bytes(3, layer));
}

describe('mvt', () => {
  test('values: strings, doubles, negative int64, sint64, uint64, booleans', () => {
    const values = [
      string(1, 'Paris'),
      double(3, 1.5),
      uint(4, -1n),
      uint(4, -123456789n),
      uint(6, zigzag(-42)),
      uint(5, 2 ** 40),
      uint(7, 1),
    ].flatMap((v) => bytes(4, v));
    const keys = ['name', 'd', 'neg', 'big', 'sint', 'uint', 'bool'].flatMap((k) => string(3, k));
    const tags = [0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6].flatMap((i) => varint(i));
    const feature = [
      ...bytes(2, tags),
      ...uint(3, 1),
      ...bytes(4, [...command(1, 1), ...varint(zigzag(10)), ...varint(zigzag(20))]),
    ];
    // A field after the 10-byte varints: read right only if they were consumed whole
    const layer = [...string(1, 'places'), ...keys, ...values, ...bytes(2, feature), ...uint(5, 512)];
    const decoded = decode(tile(layer)).get('places');
    expect(decoded?.extent).toBe(512);
    expect(decoded?.features[0]?.properties).toEqual({
      name: 'Paris',
      d: 1.5,
      neg: -1,
      big: -123456789,
      sint: -42,
      uint: 2 ** 40,
      bool: true,
    });
    expect(decoded?.features[0]?.geometry).toEqual([[10, 20]]);
  });

  test('a 10-byte varint in an unknown field keeps the next fields in step', () => {
    const layer = [...uint(9, -5n), ...string(1, 'roads'), ...uint(5, 4096)];
    expect([...decode(tile(layer)).keys()]).toEqual(['roads']);
  });

  test('polygons: MoveTo, LineTo, ClosePath with zigzag deltas', () => {
    const geometry = [
      ...command(1, 1),
      ...varint(zigzag(0)),
      ...varint(zigzag(0)),
      ...command(2, 2),
      ...varint(zigzag(10)),
      ...varint(zigzag(0)),
      ...varint(zigzag(0)),
      ...varint(zigzag(10)),
      ...command(7, 1),
      // A hole, from the last point (10, 10)
      ...command(1, 1),
      ...varint(zigzag(-5)),
      ...varint(zigzag(-3)),
      ...command(2, 1),
      ...varint(zigzag(1)),
      ...varint(zigzag(-1)),
      ...command(7, 1),
    ];
    const feature = [...uint(3, 3), ...bytes(4, geometry)];
    const decoded = decode(tile([...string(1, 'water'), ...bytes(2, feature)])).get('water');
    expect(decoded?.features[0]?.type).toBe(3);
    expect(decoded?.features[0]?.geometry).toEqual([
      [0, 0, 10, 0, 10, 10, 0, 0],
      [5, 7, 6, 6, 5, 7],
    ]);
  });

  test('only the wanted layers', () => {
    const bytes = Uint8Array.from([...tile(string(1, 'a')), ...tile(string(1, 'b'))]);
    expect([...decode(bytes, new Set(['b'])).keys()]).toEqual(['b']);
  });

  test('truncated buffers end without hanging or throwing', () => {
    const geometry = [...command(2, 1000000), ...varint(zigzag(1))];
    const feature = [...uint(3, 2), ...bytes(4, geometry)];
    const full = tile([...string(1, 'lines'), ...double(3, 0), ...bytes(2, feature)]);
    for (let n = 0; n < full.length; n += 1) expect(() => decode(full.subarray(0, n))).not.toThrow();
    // A length far past the end
    expect(() => decode(Uint8Array.from([...tag(3, 2), ...varint(2 ** 40), 1, 2]))).not.toThrow();
  });
});
