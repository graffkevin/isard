import { afterEach, describe, expect, test } from 'bun:test';
import { assetUrl } from '../src/engine-loader.ts';

afterEach(() => {
  delete (globalThis as { isardAssetsUrl?: string }).isardAssetsUrl;
});

describe('assetUrl', () => {
  test('its own option first, then the folder given, then the page’s, else next to the module', () => {
    expect(String(assetUrl('isard_bg.wasm', '/x/engine.wasm', 'https://cdn.example.org/isard'))).toBe('/x/engine.wasm');
    expect(String(assetUrl('decoder.worker.js', undefined, 'https://cdn.example.org/isard'))).toBe(
      'https://cdn.example.org/isard/decoder.worker.js',
    );
    (globalThis as { isardAssetsUrl?: string }).isardAssetsUrl = 'https://app.example.org/isard/';
    expect(String(assetUrl('isard_core_bg.wasm', undefined))).toBe('https://app.example.org/isard/isard_core_bg.wasm');
    delete (globalThis as { isardAssetsUrl?: string }).isardAssetsUrl;
    expect(String(assetUrl('basemap.worker.js', undefined))).toMatch(/\/basemap\.worker\.js$/);
  });
});
