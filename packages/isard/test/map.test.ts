/** The map without WebGL: a recording engine, a minimal DOM and frames run by hand */
import { afterAll, afterEach, beforeEach, describe, expect, mock, test } from 'bun:test';
import type { GeoMapOptions } from '../src/types.ts';
import type { PickResult } from '../src/wasm/isard.js';
import {
  domGlobals,
  EMPTY_WASM,
  FakeElement,
  type FakeEngine,
  FakeResizeObserver,
  Frames,
  setGlobals,
  wasm,
} from './fakes.ts';

const frames = new Frames();
const reported: unknown[] = [];
let fetchStatus = 200;
const fetchMock = mock(async (_url: unknown) => new Response(EMPTY_WASM, { status: fetchStatus }));

/** A stand-in for Worker: the tile loader only creates its workers with the first tile */
class NoWorker {}

const restore = setGlobals({
  ...domGlobals(frames),
  fetch: fetchMock,
  reportError: (error: unknown) => reported.push(error),
  Worker: NoWorker,
});
afterAll(restore);

const { GeoMap, defaultEasing } = await import('../src/map.ts');
const { TileLoader } = await import('../src/loader.ts');
const { TERRAIN_URL } = await import('../src/basemaps.ts');

type Map = Awaited<ReturnType<typeof GeoMap.create>>;

/** Maps made by a test, destroyed after it: they share the frame scheduler */
const maps: Map[] = [];
afterEach(() => {
  for (const map of maps.splice(0)) map.destroy();
});

/** A map with no basemap, no controls and no workers unless asked */
async function create(options: GeoMapOptions = {}): Promise<{ map: Map; engine: FakeEngine; container: FakeElement }> {
  const container = new FakeElement('div');
  const map = await GeoMap.create(container as unknown as HTMLElement, {
    basemap: 'none',
    hillshade: 0,
    attributionControl: false,
    workers: false,
    ...options,
  });
  maps.push(map);
  return { map, engine: map.engine as unknown as FakeEngine, container };
}

const canvasOf = (map: Map) => map.canvas as unknown as FakeElement;

beforeEach(() => {
  frames.flush();
  reported.length = 0;
});

describe('loading the engine', () => {
  // First in the file: a successful load is kept for the page
  test('a failed download is reported with its status, and tried again by the next map', async () => {
    // The decoding worker's tests call init too (the module mock is shared)
    const inits = wasm.init.mock.calls.length;
    fetchStatus = 404;
    await expect(create({ wasmUrl: 'engine.wasm' })).rejects.toThrow(
      'isard: failed to load the engine from engine.wasm: 404',
    );
    fetchStatus = 200;
    const { map } = await create({ wasmUrl: 'engine.wasm' });
    expect(fetchMock).toHaveBeenCalledTimes(2);
    expect(wasm.init.mock.calls.length - inits).toBe(1);
    expect(wasm.init.mock.calls.at(-1)?.[0]).toMatchObject({ module_or_path: expect.any(WebAssembly.Module) });
    // Compiled once per page
    await create({ wasmUrl: 'engine.wasm' });
    expect(fetchMock).toHaveBeenCalledTimes(2);
    map.destroy();
  });

  test('the engine without 3D Tiles by default, each engine compiled once per page', async () => {
    // The full engine was compiled by the test above (wasmUrl alone): tiles3d does not fetch it again
    const urls = () => fetchMock.mock.calls.map(([url]) => String(url));
    const before = urls().length;
    await create({ coreWasmUrl: 'core.wasm' });
    expect(urls().slice(before)).toEqual(['core.wasm']);
    await create({ coreWasmUrl: 'core.wasm' });
    await create({ tiles3d: true, coreWasmUrl: 'core-unused.wasm' });
    expect(urls().slice(before)).toEqual(['core.wasm']);
  });

  test('decoding workers: a tile loader given to the full engine, unless turned off', async () => {
    const off = await create({ workers: false });
    expect(off.engine.args('setTileLoader')).toEqual([]);
    // Only the full engine decodes 3D Tiles, in the workers
    const on = await create({ tiles3d: true, workers: 2 });
    expect(on.engine.last('setTileLoader')?.[0]).toBeInstanceOf(TileLoader);
    const none = await create({ workers: 0 });
    expect(none.engine.args('setTileLoader')).toEqual([]);
  });
});

describe('options', () => {
  test('defaults reach the engine', async () => {
    const { engine, map } = await create();
    // The Earth, WGS 84, first: the layers' shaders may take its size
    expect(engine.calls[0]).toEqual(['setBody', [6378137, 1 / 298.257223563, true]]);
    expect(engine.last('setZoomRange')).toEqual([0, 22]);
    expect(engine.last('setMaxPitch')).toEqual([70]);
    expect(engine.last('setGlobe')).toEqual([true]);
    expect(engine.last('setSky')).toEqual([true]);
    expect(engine.last('setAtmosphere')).toEqual([undefined]);
    expect(engine.last('setTerrain')).toEqual([undefined, 1, undefined, undefined]);
    expect(engine.last('setImmediateLoading')).toEqual([true]);
    const background = engine.last('setBackground') as number[];
    expect(background.map((v) => Math.round(v * 255))).toEqual([0xf2, 0xf2, 0xf0]);
    // The default camera, the canvas sized at the device pixel ratio
    expect(map.getCamera()).toEqual({ center: [2.4246, 48.8457], zoom: 13, pitch: 0, bearing: 0 });
    expect(engine.last('resize')).toEqual([800, 600, 2]);
    expect([canvasOf(map).width, canvasOf(map).height]).toEqual([1600, 1200]);
  });

  test('given options are normalized', async () => {
    const { engine, map } = await create({
      minZoom: 3,
      maxZoom: 12,
      maxPitch: 45,
      projection: 'mercator',
      sky: false,
      loading: 'progressive' as never,
      background: '#ff0000',
      terrain: { exaggeration: 2 },
      center: [5, 45],
      zoom: 8,
    });
    expect(engine.last('setZoomRange')).toEqual([3, 12]);
    expect(engine.last('setMaxPitch')).toEqual([45]);
    expect(engine.last('setGlobe')).toEqual([false]);
    expect(engine.last('setSky')).toEqual([false]);
    expect(engine.last('setImmediateLoading')).toEqual([false]);
    expect(engine.last('setBackground')).toEqual([1, 0, 0]);
    expect(engine.last('setTerrain')).toEqual([TERRAIN_URL, 2, 15, true]);
    expect(map.getCamera()).toMatchObject({ center: [5, 45], zoom: 8 });
  });

  test('terrain: true takes every default; setTerrain(false) removes it', async () => {
    const { engine, map } = await create({ terrain: true });
    expect(engine.last('setTerrain')).toEqual([TERRAIN_URL, 1, 15, true]);
    map.setTerrain(false);
    expect(engine.last('setTerrain')).toEqual([undefined, 1, undefined, undefined]);
  });

  test('atmosphere: colors as RGB 0-1, then the three blends', async () => {
    const { engine, map } = await create({ atmosphere: { skyColor: '#ff0000', fogGroundBlend: 0.25 } });
    const sky = engine.last('setAtmosphere')?.[0] as Float32Array;
    expect(sky).toBeInstanceOf(Float32Array);
    expect(sky.length).toBe(12);
    expect([...sky.slice(0, 3)]).toEqual([1, 0, 0]);
    expect(sky[9]).toBeCloseTo(0.6, 6);
    expect(sky[10]).toBeCloseTo(0.7, 6);
    expect(sky[11]).toBeCloseTo(0.25, 6);
    map.setAtmosphere(undefined);
    expect(engine.last('setAtmosphere')).toEqual([undefined]);
  });

  test('an unknown background color is black', async () => {
    const { engine, map } = await create();
    map.setBackground('not a color');
    expect(engine.last('setBackground')).toEqual([0, 0, 0]);
  });

  test('the Moon: its sphere, no atmosphere, none of the Géoplateforme defaults', async () => {
    // The map's own defaults, not those of `create`
    const container = new FakeElement('div');
    const map = await GeoMap.create(container as unknown as HTMLElement, {
      body: 'moon',
      workers: false,
      attributionControl: { customAttribution: 'NASA' },
    });
    maps.push(map);
    const engine = map.engine as unknown as FakeEngine;
    expect(engine.last('setBody')).toEqual([1737400, 0, false]);
    expect(engine.args('addRaster')).toEqual([]);
    expect(engine.last('setBackground')).toEqual([0, 0, 0]);
    expect((engine.last('setGlobeColor') as number[]).map((v) => Math.round(v * 255))).toEqual([0x77, 0x75, 0x6f]);
    expect(map.getCamera()).toEqual({ center: [0, 0], zoom: 2, pitch: 0, bearing: 0 });
    // The credits of the page only
    const credits = (element: FakeElement): FakeElement | undefined =>
      element.className === 'isard-attrib-inner' ? element : element.children.map(credits).find(Boolean);
    expect(credits(container)?.innerHTML).toBe('NASA');
    // No IGN relief there: a url is needed, its altitudes left as served
    expect(() => map.setTerrain(true)).toThrow(/needs a url/);
    map.setTerrain({ url: 'https://moon.example/{z}/{x}/{y}.bil' });
    expect(engine.last('setTerrain')).toEqual(['https://moon.example/{z}/{x}/{y}.bil', 1, 15, false]);
  });

  test('another body: its radius and flattening, its atmosphere and color', async () => {
    const { engine } = await create({
      body: { radius: 3396190, flattening: 0.00589, atmosphere: true, color: '#c1440e' },
    });
    expect(engine.last('setBody')).toEqual([3396190, 0.00589, true]);
    expect(engine.last('setGlobeColor')).toEqual([0xc1 / 255, 0x44 / 255, 0x0e / 255]);
    const { engine: sphere } = await create({ body: { radius: 1000 } });
    expect(sphere.last('setBody')).toEqual([1000, 0, false]);
  });

  test('an equirectangular raster is reprojected by a loader, one level finer in Web Mercator', async () => {
    const { engine, map } = await create();
    map.addRaster({ url: 'https://trek.example/{z}/{y}/{x}.jpg', grid: 'equirectangular', maxZoom: 8 });
    const [url, minZoom, maxZoom, , , size, loader] = engine.last('addRaster') ?? [];
    expect([url, minZoom, maxZoom, size]).toEqual(['https://trek.example/{z}/{y}/{x}.jpg', 0, 9, undefined]);
    expect(loader).toBeInstanceOf(Function);
    map.addRaster({ url: 'https://tiles.example/{z}/{x}/{y}.png' });
    expect(engine.last('addRaster')?.slice(2)).toEqual([19, 1, 1, undefined, undefined]);
  });

  test('setMinZoom and setMaxZoom keep the other end', async () => {
    const { engine, map } = await create({ minZoom: 2, maxZoom: 18 });
    map.setMinZoom(4);
    expect(engine.last('setZoomRange')).toEqual([4, 18]);
    map.setMaxZoom(10);
    expect(engine.last('setZoomRange')).toEqual([4, 10]);
  });
});

describe('keyboard and accessibility', () => {
  test('by default the canvas is focusable and the container listens to the keys', async () => {
    const { map, container } = await create({ ariaLabel: 'Flood map' });
    const canvas = canvasOf(map);
    expect(canvas.getAttribute('role')).toBe('application');
    expect(canvas.getAttribute('aria-roledescription')).toBe('map');
    expect(canvas.getAttribute('aria-label')).toBe('Flood map');
    expect(canvas.tabIndex).toBe(0);
    expect(container.listeners.get('keydown')?.size).toBe(1);
  });

  test('keyboard: false keeps the gestures, not the keys', async () => {
    const { map, container } = await create({ keyboard: false });
    const canvas = canvasOf(map);
    expect(canvas.getAttribute('role')).toBe('region');
    expect(canvas.getAttribute('aria-label')).toBe('Map');
    expect(canvas.tabIndex).toBe(-1);
    expect(container.listeners.get('keydown')?.size ?? 0).toBe(0);
    expect(canvas.listeners.get('pointerdown')?.size).toBe(1);
  });

  test('a map that is not interactive takes no key whatever the keyboard option', async () => {
    const { map, container } = await create({ interactive: false, keyboard: true });
    expect(canvasOf(map).getAttribute('role')).toBe('region');
    expect(canvasOf(map).tabIndex).toBe(-1);
    expect(container.listeners.get('keydown')?.size ?? 0).toBe(0);
    expect(canvasOf(map).listeners.get('pointerdown')?.size ?? 0).toBe(0);
  });
});

describe('events', () => {
  test('on returns the function that stops listening', async () => {
    const { map } = await create();
    const seen: number[] = [];
    const off = map.on('render', ({ ms }) => seen.push(ms));
    map.emit('render', { ms: 1 });
    off();
    map.emit('render', { ms: 2 });
    expect(seen).toEqual([1]);
    // Twice is harmless, and other listeners stay
    off();
    const other = mock(() => {});
    map.on('render', other);
    map.emit('render', { ms: 3 });
    expect(other).toHaveBeenCalledTimes(1);
    expect(seen).toEqual([1]);
  });

  test('a listener that throws is reported and does not stop the others', async () => {
    const { map } = await create();
    const error = new Error('listener');
    const after = mock(() => {});
    map.on('idle', () => {
      throw error;
    });
    map.on('idle', after);
    map.emit('idle', {});
    expect(after).toHaveBeenCalledTimes(1);
    expect(reported).toEqual([error]);
  });

  test('movestart, move and moveend around a camera change; idle once settled', async () => {
    const { map } = await create();
    const seen: string[] = [];
    for (const type of ['movestart', 'move', 'moveend', 'idle'] as const) map.on(type, () => seen.push(type));
    map.setCamera({ zoom: 5 });
    frames.flush();
    expect(seen).toEqual(['movestart', 'move', 'moveend', 'idle']);
    // Nothing changed: no event
    seen.length = 0;
    map.requestRender();
    frames.flush();
    expect(seen).toEqual([]);
  });

  test('no idle while tiles load', async () => {
    const { map, engine } = await create();
    const idle = mock(() => {});
    map.on('idle', idle);
    engine.loading = true;
    map.requestRender();
    frames.flush();
    expect(idle).not.toHaveBeenCalled();
    engine.loading = false;
    map.requestRender();
    frames.flush();
    expect(idle).toHaveBeenCalledTimes(1);
  });

  test('requestRender coalesces into one frame', async () => {
    const { map, engine } = await create();
    const before = engine.args('render').length;
    map.requestRender();
    map.requestRender();
    map.requestRender();
    expect(frames.pending).toBe(1);
    frames.flush();
    expect(engine.args('render').length).toBe(before + 1);
  });

  test('a frame that throws does not stop the next ones', async () => {
    const { map, engine } = await create();
    const render = mock(() => {});
    map.on('render', render);
    map.on('move', () => {
      throw new Error('in a frame');
    });
    map.setCamera({ zoom: 3 });
    frames.run();
    expect(reported.length).toBe(1);
    map.requestRender();
    frames.flush();
    expect(engine.args('render').length).toBeGreaterThan(0);
    expect(render).toHaveBeenCalled();
  });
});

describe('hover', () => {
  const hit: PickResult = { x: 10, y: 20, lonLat: [1, 2] };

  test("'hover' comes with the next frame, with the position under the pointer", async () => {
    const { map } = await create();
    const hover = mock((_e: unknown) => {});
    map.on('hover', hover);
    map.hover([30, 40]);
    expect(hover).not.toHaveBeenCalled();
    frames.run();
    expect(hover).toHaveBeenCalledWith({ point: [30, 40], lonLat: [3, 4] });
    // Once per pointer position
    map.requestRender();
    frames.flush();
    expect(hover).toHaveBeenCalledTimes(1);
  });

  test('no picking while nobody listens to hoverfeature', async () => {
    const { map, engine } = await create();
    map.on('hover', () => {});
    map.hover([1, 1]);
    frames.flush();
    expect(engine.args('requestPick')).toEqual([]);
  });

  test('hoverfeature: asked after the frame with the hoverPicking options, polled until the answer', async () => {
    const { map, engine } = await create();
    const line = map.addLines({
      data: {
        type: 'Feature',
        geometry: {
          type: 'LineString',
          coordinates: [
            [0, 0],
            [1, 1],
          ],
        },
        properties: { n: 1 },
      },
    });
    map.hoverPicking = { radius: 5, layers: [line] };
    const found = mock((_e: unknown) => {});
    const off = map.on('hoverfeature', found);
    map.hover([10, 20]);
    frames.run();
    expect(engine.last('requestPick')).toEqual([10, 20, 5, Uint32Array.of(line.id)]);
    // Not ready on the first poll: polled again, without a frame drawn
    engine.polls = [undefined, { ...hit, hit: { layer: line.id, index: 0 } }];
    const renders = engine.args('render').length;
    frames.run();
    expect(found).not.toHaveBeenCalled();
    frames.run();
    expect(engine.args('render').length).toBe(renders);
    expect(found).toHaveBeenCalledTimes(1);
    const event = found.mock.calls[0]?.[0] as {
      point: unknown;
      lonLat: unknown;
      hit: { layer: unknown; index: number; feature: { properties: unknown } };
    };
    expect(event.point).toEqual([10, 20]);
    expect(event.lonLat).toEqual([1, 2]);
    expect(event.hit.layer).toBe(line);
    expect(event.hit.index).toBe(0);
    expect(event.hit.feature.properties).toEqual({ n: 1 });
    expect(frames.pending).toBe(0);
    // Not listened to any more: no picking
    off();
    map.hover([11, 21]);
    frames.flush();
    expect(engine.args('requestPick').length).toBe(1);
  });

  test('a hit on a removed layer has no feature hit', async () => {
    const { map, engine } = await create();
    const found = mock((_e: unknown) => {});
    map.on('hoverfeature', found);
    map.hover([1, 1]);
    frames.run();
    engine.polls = [{ ...hit, hit: { layer: 999, index: 3 } }];
    frames.run();
    expect(found.mock.calls[0]?.[0]).toMatchObject({ hit: undefined });
  });

  test('one poll loop at most: a new pointer position replaces the request in flight', async () => {
    const { map, engine } = await create();
    map.on('hoverfeature', () => {});
    map.hover([1, 1]);
    frames.run();
    map.hover([2, 2]);
    // The poll and the frame
    expect(frames.pending).toBe(2);
    frames.run();
    expect(engine.args('requestPick').length).toBe(2);
    expect(frames.pending).toBe(1);
  });

  test('no poll when the engine cannot pick', async () => {
    const { map, engine } = await create();
    map.on('hoverfeature', () => {});
    engine.canPick = false;
    map.hover([1, 1]);
    frames.run();
    expect(engine.args('requestPick').length).toBe(1);
    frames.flush();
    expect(engine.args('pollPick')).toEqual([]);
  });

  test('hover(undefined) (the pointer left) cancels the picking under way', async () => {
    const { map, engine } = await create();
    const found = mock(() => {});
    map.on('hoverfeature', found);
    map.hover([1, 1]);
    frames.run();
    engine.polls = [hit];
    map.hover(undefined);
    frames.flush();
    expect(engine.args('pollPick')).toEqual([]);
    expect(found).not.toHaveBeenCalled();
  });

  test('a mouse leaving the canvas ends the hover and sends leave', async () => {
    const { map, engine } = await create();
    const leave = mock(() => {});
    map.on('leave', leave);
    map.on('hoverfeature', () => {});
    map.hover([1, 1]);
    frames.run();
    canvasOf(map).dispatch('pointerleave', { pointerType: 'mouse' });
    expect(leave).toHaveBeenCalledTimes(1);
    frames.flush();
    expect(engine.args('pollPick')).toEqual([]);
  });

  test('destroy cancels the picking under way', async () => {
    const { map, engine } = await create();
    map.on('hoverfeature', () => {});
    map.hover([1, 1]);
    frames.run();
    map.destroy();
    frames.flush();
    expect(engine.args('pollPick')).toEqual([]);
  });
});

describe('camera', () => {
  test('setCamera merges a partial camera', async () => {
    const { map } = await create({ center: [1, 2], zoom: 3, pitch: 10, bearing: 20 });
    map.setCamera({ zoom: 6 });
    expect(map.getCamera()).toEqual({ center: [1, 2], zoom: 6, pitch: 10, bearing: 20 });
  });

  test('getBounds: the engine bounds, the whole world if it gives none', async () => {
    const { map, engine } = await create();
    expect(map.getBounds()).toEqual([1, 2, 3, 4]);
    engine.boundsValue = new Float64Array(0);
    expect(map.getBounds()).toEqual([-180, -85, 180, 85]);
  });

  test('easeTo turns the short way and ends on the camera asked for', async () => {
    const { map, engine } = await create({ bearing: 350 });
    map.easeTo({ bearing: 10, zoom: 15 }, 100);
    expect(engine.last('preload')).toEqual([2.4246, 48.8457, 15, 0, 10]);
    const start = performance.now();
    frames.run(start + 50);
    const middle = map.getCamera();
    // From 350 through 0: past 350, not down through 180
    expect(middle.bearing > 350 || middle.bearing < 10).toBe(true);
    expect(middle.zoom).toBeGreaterThan(13);
    expect(middle.zoom).toBeLessThan(15);
    frames.flush(50, start + 1000);
    const end = map.getCamera();
    expect(end.zoom).toBe(15);
    expect(((end.bearing % 360) + 360) % 360).toBeCloseTo(10, 9);
    expect(engine.args('clearPreload').length).toBeGreaterThan(0);
  });

  test('easeTo with no duration jumps at the next frame', async () => {
    const { map } = await create();
    map.easeTo({ center: [10, 20] }, { duration: 0 });
    frames.run();
    expect(map.getCamera().center).toEqual([10, 20]);
  });

  test('a new animation replaces the running one and drops its preload', async () => {
    const { map, engine } = await create();
    map.easeTo({ zoom: 5 }, 1000);
    const cleared = engine.args('clearPreload').length;
    map.easeTo({ zoom: 7 }, 0);
    expect(engine.args('clearPreload').length).toBe(cleared + 1);
    frames.flush();
    expect(map.getCamera().zoom).toBe(7);
  });

  test('flyTo zooms out on a long move and ends on the camera asked for', async () => {
    const { map } = await create({ center: [2, 48], zoom: 12 });
    map.flyTo({ center: [-74, 40.7], zoom: 12, pitch: 40 }, { duration: 1000 });
    const start = performance.now();
    frames.run(start + 500);
    expect(map.getCamera().zoom).toBeLessThan(8);
    frames.flush(50, start + 2000);
    const end = map.getCamera();
    expect(end.zoom).toBe(12);
    expect(end.pitch).toBeCloseTo(40, 9);
    expect(end.center[0]).toBeCloseTo(-74, 6);
    expect(end.center[1]).toBeCloseTo(40.7, 6);
  });

  test('flyTo crosses the antimeridian the short way', async () => {
    const { map } = await create({ center: [170, 0], zoom: 4 });
    map.flyTo({ center: [-170, 0] }, { duration: 1000 });
    const start = performance.now();
    for (const t of [100, 300, 500, 700, 900]) {
      frames.run(start + t);
      const lon = map.getCamera().center[0];
      expect(Math.abs(lon)).toBeGreaterThan(165);
    }
    frames.flush(50, start + 2000);
    expect(map.getCamera().center[0]).toBeCloseTo(-170, 6);
  });

  test('flyTo without a move on the ground is a plain zoom', async () => {
    const { map } = await create({ center: [2, 48], zoom: 10 });
    map.flyTo({ zoom: 14 }, { duration: 1000 });
    const start = performance.now();
    frames.run(start + 500);
    const middle = map.getCamera();
    expect(middle.center[0]).toBeCloseTo(2, 9);
    expect(middle.zoom).toBeGreaterThan(10);
    expect(middle.zoom).toBeLessThan(14);
    frames.flush(50, start + 2000);
    expect(map.getCamera().zoom).toBe(14);
  });
});

describe('defaultEasing', () => {
  test('clamped to 0 and 1 at the ends', () => {
    expect(defaultEasing(0)).toBe(0);
    expect(defaultEasing(1)).toBe(1);
    expect(defaultEasing(-1)).toBe(0);
    expect(defaultEasing(2)).toBe(1);
  });

  test("CSS's ease curve: fast then slow, increasing", () => {
    // cubic-bezier(0.25, 0.1, 0.25, 1) at 0.5
    expect(defaultEasing(0.5)).toBeCloseTo(0.8024, 3);
    let previous = 0;
    for (let i = 1; i <= 100; i += 1) {
      const v = defaultEasing(i / 100);
      expect(v).toBeGreaterThanOrEqual(previous);
      previous = v;
    }
  });
});

describe('networks', () => {
  const nodes = Float64Array.of(0, 0, 1, 1, 2, 2);
  const costs = new Float32Array(3);

  test('nodes must be pairs, edges pairs of existing nodes', async () => {
    const { map } = await create();
    expect(() => map.addNetwork({ nodes: Float64Array.of(0, 0, 1), edges: Uint32Array.of(0, 1), costs })).toThrow(
      'network nodes must be lon, lat pairs (length 3)',
    );
    expect(() => map.addNetwork({ nodes, edges: Uint32Array.of(0, 1, 2), costs })).toThrow(
      'network edges must be node index pairs (length 3)',
    );
    expect(() => map.addNetwork({ nodes, edges: Uint32Array.of(0, 1, 1, 3), costs })).toThrow(
      'network edge 1 refers to node 3, of 3 nodes',
    );
  });
});

describe('context loss', () => {
  test('nothing is drawn while lost; a new engine gets the settings, the camera and the layers back', async () => {
    const { map, engine } = await create({ maxPitch: 50, projection: 'mercator', body: 'moon' });
    const line = map.addLines({
      data: { type: 'Feature', geometry: { type: 'Point', coordinates: [0, 0] }, properties: {} },
    });
    map.setMinZoom(3);
    map.setCamera({ center: [7, 8], zoom: 9 });
    frames.flush();
    canvasOf(map).dispatch('webglcontextlost');
    const renders = engine.args('render').length;
    map.requestRender();
    frames.flush();
    expect(engine.args('render').length).toBe(renders);
    canvasOf(map).dispatch('webglcontextrestored');
    const fresh = map.engine as unknown as FakeEngine;
    expect(fresh).not.toBe(engine);
    expect(engine.freed).toBe(true);
    expect(fresh.calls[0]).toEqual(['setBody', [1737400, 0, false]]);
    expect(fresh.last('setZoomRange')).toEqual([3, 22]);
    expect(fresh.last('setMaxPitch')).toEqual([50]);
    expect(fresh.last('setGlobe')).toEqual([false]);
    expect(map.getCamera()).toMatchObject({ center: [7, 8], zoom: 9 });
    expect(fresh.args('addLines').length + fresh.args('addCircles').length).toBe(1);
    expect(line.id).toBeGreaterThan(0);
  });
});

describe('destroy', () => {
  test('frees the engine, leaves one that does nothing but keep the camera, and clears the DOM and listeners', async () => {
    const { map, engine, container } = await create({ center: [3, 4], zoom: 5 });
    const observers = FakeResizeObserver.instances.length;
    const move = mock(() => {});
    map.on('move', move);
    expect(container.className).toContain('isard-map');
    map.destroy();
    const renders = engine.args('render').length;
    expect(engine.freed).toBe(true);
    expect(container.children).toEqual([]);
    expect(container.className).not.toContain('isard-map');
    expect(FakeResizeObserver.instances[observers - 1]?.disconnected).toBe(true);
    // Late callers
    expect(map.getCamera()).toMatchObject({ center: [3, 4], zoom: 5 });
    expect(map.getBounds()).toEqual([-180, -85, 180, 85]);
    expect(() => map.setCamera({ zoom: 9 })).not.toThrow();
    map.requestRender();
    frames.flush();
    expect(engine.args('render').length).toBe(renders);
    map.emit('move', map.getCamera());
    expect(move).not.toHaveBeenCalled();
  });
});
