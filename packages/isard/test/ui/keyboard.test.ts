import { describe, expect, test } from 'bun:test';
import { keyAction, keyCamera } from '../../src/ui/interaction.ts';
import type { Camera } from '../../src/wasm/isard.js';

const camera = (c: Partial<Camera> = {}) => ({ center: [2, 48], zoom: 10, pitch: 0, bearing: 0, ...c }) as Camera;

describe('keyAction', () => {
  test('arrows pan, Shift + arrows rotate and tilt', () => {
    expect(keyAction('ArrowLeft', false)).toEqual({ pan: [-100, 0] });
    expect(keyAction('ArrowDown', false)).toEqual({ pan: [0, 100] });
    expect(keyAction('ArrowLeft', true)).toEqual({ bearing: -15 });
    expect(keyAction('ArrowUp', true)).toEqual({ pitch: 10 });
  });

  test('zoom keys, a bigger step with Shift', () => {
    expect(keyAction('+', false)).toEqual({ zoom: 1 });
    expect(keyAction('=', false)).toEqual({ zoom: 1 });
    expect(keyAction('+', true)).toEqual({ zoom: 2 });
    expect(keyAction('-', false)).toEqual({ zoom: -1 });
    expect(keyAction('_', true)).toEqual({ zoom: -2 });
  });

  test('0 resets, other keys do nothing', () => {
    expect(keyAction('0', false)).toEqual({ reset: true });
    expect(keyAction('a', false)).toBeUndefined();
    expect(keyAction('Enter', false)).toBeUndefined();
  });
});

describe('keyCamera', () => {
  test('zoom, bearing, pitch and reset', () => {
    expect(keyCamera({ zoom: 2 }, camera())).toEqual({ zoom: 12 });
    expect(keyCamera({ bearing: -15 }, camera({ bearing: 30 }))).toEqual({ bearing: 15 });
    expect(keyCamera({ pitch: -10 }, camera({ pitch: 5 }))).toEqual({ pitch: 0 });
    expect(keyCamera({ reset: true }, camera({ bearing: 40, pitch: 30 }))).toEqual({ bearing: 0, pitch: 0 });
  });

  test('a pan moves the center by that many pixels, north up', () => {
    const c = camera({ center: [0, 0], zoom: 0 });
    // The world is 512 px at zoom 0: 128 px east is 90 degrees
    const east = keyCamera({ pan: [128, 0] }, c).center ?? [0, 0];
    expect(east[0]).toBeCloseTo(90, 6);
    expect(east[1]).toBeCloseTo(0, 6);
    const north = keyCamera({ pan: [0, -100] }, camera()).center ?? [0, 0];
    expect(north[0]).toBeCloseTo(2, 9);
    expect(north[1]).toBeGreaterThan(48);
  });

  test('the bearing turns the pan: up goes east when facing east', () => {
    const center = keyCamera({ pan: [0, -100] }, camera({ bearing: 90 })).center ?? [0, 0];
    expect(center[0]).toBeGreaterThan(2);
    expect(center[1]).toBeCloseTo(48, 9);
  });

  test('the tilt stretches vertical pans; longitudes wrap', () => {
    const flat = keyCamera({ pan: [0, -100] }, camera()).center ?? [0, 0];
    const tilted = keyCamera({ pan: [0, -100] }, camera({ pitch: 60 })).center ?? [0, 0];
    expect(tilted[1] - 48).toBeCloseTo((flat[1] - 48) * 2, 3);
    const wrapped = keyCamera({ pan: [256, 0] }, camera({ center: [179, 0], zoom: 0 })).center ?? [0, 0];
    expect(wrapped[0]).toBeCloseTo(-1, 6);
  });
});
