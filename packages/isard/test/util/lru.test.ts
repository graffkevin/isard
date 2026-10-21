/** The shared helpers of the page and the workers: Web Mercator, the cache of the most recently used entries */
import { describe, expect, test } from 'bun:test';
import { fromMercator, latitude, mercatorX, mercatorY, toMercator } from '../../src/util/geo.ts';
import { Lru } from '../../src/util/lru.ts';

describe('Web Mercator', () => {
  test('the corners of the world, its center, and back', () => {
    expect(toMercator([-180, 85.051128779806589])).toEqual([0, expect.closeTo(0, 12)]);
    expect(toMercator([0, 0])).toEqual([0.5, 0.5]);
    expect(mercatorX(180)).toBe(1);
    // Past the edges: clamped
    expect(mercatorY(90)).toBeCloseTo(0, 12);
    expect(mercatorY(-90)).toBeCloseTo(1, 12);
    const [lon, lat] = fromMercator(toMercator([2.35, 48.85]));
    expect(lon).toBeCloseTo(2.35, 12);
    expect(lat).toBeCloseTo(48.85, 12);
    expect(latitude(0.5)).toBe(0);
  });
});

describe('Lru', () => {
  test('the least recently used dropped past the capacity, told to `evict`; a read or a write uses an entry', () => {
    const evicted: [string, number][] = [];
    const lru = new Lru<string, number>(2, (value, key) => evicted.push([key, value]));
    lru.set('a', 1);
    lru.set('b', 2);
    expect(lru.get('a')).toBe(1);
    lru.set('c', 3);
    expect(evicted).toEqual([['b', 2]]);
    expect([...lru.values()]).toEqual([1, 3]);
    // `has` does not use it
    expect(lru.has('a')).toBe(true);
    lru.set('d', 4);
    expect(evicted).toEqual([
      ['b', 2],
      ['a', 1],
    ]);
    // Deleted: not evicted
    expect(lru.delete('c')).toBe(true);
    expect(evicted.length).toBe(2);
    expect(lru.size).toBe(1);
  });

  test('a capacity lowered applies at the next write', () => {
    const lru = new Lru<number, number>(4);
    [0, 1, 2, 3].forEach((i) => {
      lru.set(i, i);
    });
    lru.capacity = 2;
    expect(lru.size).toBe(4);
    lru.set(4, 4);
    expect([...lru.values()]).toEqual([3, 4]);
  });
});
