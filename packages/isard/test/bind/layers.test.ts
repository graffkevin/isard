/** What the framework bindings do when a layer's description changes, and the order of their layers */
import { describe, expect, test } from 'bun:test';
import { applyLayerChange, LayerOrder, layerChange } from '../../src/bind/layers.ts';
import type { Layer } from '../../src/layers/layers.ts';
import type { LayerSpec } from '../../src/map.ts';

const data = { type: 'FeatureCollection' as const, features: [] };

describe('layerChange', () => {
  test('nothing changed: values compared by identity', () => {
    const spec: LayerSpec = { type: 'lines', data, color: '#000', width: 2 };
    expect(layerChange(spec, { ...spec })).toBeUndefined();
  });

  test('the data and the style of a feature layer: updated', () => {
    const next = { type: 'lines', data: { ...data }, color: '#f00' } as const;
    expect(layerChange({ type: 'lines', data, color: '#000' }, next)).toEqual({
      rebuild: false,
      style: { color: '#f00' },
      own: { data: next.data },
    });
  });

  test('what is set at creation, a key taken away, another type: made anew', () => {
    const raster: LayerSpec = { type: 'raster', url: 'a/{z}/{x}/{y}.png', opacity: 1 };
    expect(layerChange(raster, { ...raster, opacity: 0.5 })).toEqual({
      rebuild: false,
      style: { opacity: 0.5 },
      own: {},
    });
    expect(layerChange(raster, { ...raster, url: 'b/{z}/{x}/{y}.png' })).toEqual({ rebuild: true });
    expect(layerChange(raster, { type: 'raster', url: raster.url })).toEqual({ rebuild: true });
    expect(layerChange(raster, { type: 'tiles3d', url: 't.json' })).toEqual({ rebuild: true });
    const tiles: LayerSpec = { type: 'tiles3d', url: 't.json' };
    expect(layerChange(tiles, { ...tiles, color: '#ccc' })).toEqual({
      rebuild: false,
      style: { color: '#ccc' },
      own: {},
    });
    expect(layerChange(tiles, { ...tiles, geoid: true })).toEqual({ rebuild: true });
  });

  test('the instances of models and the uniforms of a ground shader: their own setters', () => {
    const instances = [{ lonLat: [2, 48] as [number, number] }];
    const models: LayerSpec = { type: 'models', url: 'm.glb', instances: [] };
    expect(layerChange(models, { ...models, instances })).toEqual({ rebuild: false, style: {}, own: { instances } });
  });
});

describe('applyLayerChange', () => {
  test('calls the setters of what changed', () => {
    const calls: unknown[][] = [];
    const layer = {
      setData: (d: unknown) => calls.push(['data', d]),
      setStyle: (s: unknown) => calls.push(['style', s]),
      setUniform: (n: string, v: unknown) => calls.push(['uniform', n, v]),
    } as unknown as Layer;
    const next: LayerSpec = { type: 'lines', data, color: '#f00' };
    applyLayerChange(layer, next, { rebuild: false, style: { color: '#f00' }, own: { data } });
    const ground: LayerSpec = { type: 'ground', code: '', anchor: [0, 0], uniforms: { t: 1 } };
    applyLayerChange(layer, ground, { rebuild: false, style: {}, own: { uniforms: { t: 1 } } });
    expect(calls).toEqual([
      ['data', data],
      ['style', { color: '#f00' }],
      ['uniform', 't', 1],
    ]);
  });
});

describe('LayerOrder', () => {
  /** Layers that record the layer they are moved under */
  const fake = (name: string, moves: string[]) =>
    ({ name, moveBefore: (other: { name: string }) => moves.push(`${name}<${other.name}`) }) as unknown as Layer;

  test('by order, then by arrival; a layer made anew keeps its place', () => {
    const moves: string[] = [];
    const order = new LayerOrder();
    const a = fake('a', moves);
    const b = fake('b', moves);
    const c = fake('c', moves);
    order.place(a, 1);
    order.place(b, 0); // under a
    order.place(c, 1); // above a: on top, not moved
    expect(moves).toEqual(['b<a']);
    const b2 = fake('b2', moves);
    order.place(b2, 0, b);
    expect(moves).toEqual(['b<a', 'b2<a']);
    order.remove(a);
    order.place(a, 0); // above b2, under c
    expect(moves.at(-1)).toBe('a<c');
  });
});
