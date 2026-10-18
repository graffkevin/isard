/**
 * The map's engine replaced under its layers: the engine with 3D Tiles loaded for the first 3D Tiles layer of a map
 * on the engine without them, a WebGL context lost and restored, a map destroyed meanwhile
 */
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, mock, test } from 'bun:test';
import type { Feature } from '../src/features.ts';
import type { GeoMapOptions } from '../src/types.ts';
import { CoreFakeEngine, domGlobals, EMPTY_WASM, FakeElement, FakeEngine, Frames, setGlobals } from './fakes.ts';

const frames = new Frames();
const reported: unknown[] = [];
/** The download of the full engine: held until released, failed with `fullStatus` */
let gate: Promise<void> | undefined;
let open: (() => void) | undefined;
let fullStatus = 200;
const fetchMock = mock(async (url: unknown) => {
  const full = String(url).includes('full');
  if (full && gate) await gate;
  return new Response(EMPTY_WASM, { status: full ? fullStatus : 200 });
});
const fullFetches = () => fetchMock.mock.calls.filter(([url]) => String(url).includes('full')).length;

/** Holds the next downloads of the full engine until `release()` */
function hold(): void {
  gate = new Promise((resolve) => {
    open = resolve;
  });
}

function release(): void {
  open?.();
  gate = undefined;
}

/** Lets the pending promises and timers run (the engine instantiated, the map rebuilt in a task of its own) */
async function settle(): Promise<void> {
  for (let i = 0; i < 4; i += 1) await new Promise((resolve) => setTimeout(resolve, 1));
}

const restoreGlobals = setGlobals({
  ...domGlobals(frames),
  fetch: fetchMock,
  reportError: (error: unknown) => reported.push(error),
  Worker: class {},
});

const { GeoMap, forgetEngines } = await import('../src/map.ts');

// Each test file starts with no engine loaded, and leaves none
beforeAll(forgetEngines);
afterAll(() => {
  forgetEngines();
  restoreGlobals();
});

type Map = Awaited<ReturnType<typeof GeoMap.create>>;
const maps: Map[] = [];

beforeEach(() => {
  reported.length = 0;
  fullStatus = 200;
  release();
  forgetEngines();
});
afterEach(() => {
  for (const map of maps.splice(0)) map.destroy();
  frames.flush();
});

async function create(options: GeoMapOptions = {}): Promise<Map> {
  const map = await GeoMap.create(new FakeElement('div') as unknown as HTMLElement, {
    basemap: 'none',
    attributionControl: false,
    workers: false,
    coreWasmUrl: 'core.wasm',
    wasmUrl: 'full.wasm',
    ...options,
  });
  maps.push(map);
  return map;
}

const engineOf = (map: Map) => map.engine as unknown as FakeEngine;
const canvasOf = (map: Map) => map.canvas as unknown as FakeElement;
const point: Feature = { type: 'Feature', geometry: { type: 'Point', coordinates: [0, 0] }, properties: {} };
/** The layers created in an engine, in order */
const created = (engine: FakeEngine) =>
  engine.calls
    .filter(([name]) => /^add(Lines|Circles|Fills|Tiles3d|Extrusions|Network|Raster)$/.test(name))
    .map(([n]) => n);

describe('the engine with 3D Tiles, loaded for the first 3D Tiles layer', () => {
  test('the map moves onto it with its camera, settings and layers in order', async () => {
    hold();
    const map = await create({ maxPitch: 60 });
    const core = engineOf(map);
    expect(core).toBeInstanceOf(CoreFakeEngine);
    const below = map.addLines({ data: point });
    const tiles = map.addTiles3d({ url: 'city/tileset.json' });
    const above = map.addCircles({ data: point });
    // Waiting: not in the core engine, which has no 3D Tiles
    expect(tiles.id).toBe(-1);
    expect(tiles.stats()).toBeUndefined();
    tiles.setStyle({ opacity: 0.5 });
    map.setMinZoom(3);
    map.setCamera({ center: [5, 45], zoom: 12 });
    // No idle while it loads
    const idle = mock(() => {});
    map.on('idle', idle);
    frames.flush();
    expect(idle).not.toHaveBeenCalled();
    release();
    await settle();
    const full = engineOf(map);
    expect(full).not.toBe(core);
    expect(full).not.toBeInstanceOf(CoreFakeEngine);
    expect(core.freed).toBe(true);
    expect(created(full)).toEqual(['addLines', 'addTiles3d', 'addCircles']);
    expect([below.id, tiles.id, above.id].every((id) => id >= 0)).toBe(true);
    expect(full.last('setTiles3dStyle')).toEqual([tiles.id, 16, 0.5]);
    expect(full.last('setZoomRange')).toEqual([3, 22]);
    expect(full.last('setMaxPitch')).toEqual([60]);
    expect(map.getCamera()).toMatchObject({ center: [5, 45], zoom: 12 });
    frames.flush();
    expect(idle).toHaveBeenCalled();
    expect(reported).toEqual([]);
  });

  test('layers added and moved while it loads keep their order', async () => {
    hold();
    const map = await create();
    const core = engineOf(map);
    const a = map.addLines({ data: point });
    map.addTiles3d({ url: 'city/tileset.json' });
    const b = map.addCircles({ data: point });
    // Under a: index 0 in the core engine, which does not hold the 3D Tiles layer
    b.moveBefore(a);
    expect(core.last('moveLayer')).toEqual([b.id, 0]);
    const c = map.addFills({ data: point });
    release();
    await settle();
    expect(created(engineOf(map))).toEqual(['addCircles', 'addLines', 'addTiles3d', 'addFills']);
    expect(c.id).toBeGreaterThanOrEqual(0);
  });

  test('a map destroyed while it loads stays destroyed', async () => {
    hold();
    const map = await create();
    map.addTiles3d({ url: 'city/tileset.json' });
    map.destroy();
    const engines = FakeEngine.instances.length;
    release();
    await settle();
    expect(FakeEngine.instances.length).toBe(engines);
    expect(reported).toEqual([]);
  });

  test('a failed download is reported, the layer waits; the next 3D Tiles layer tries again', async () => {
    fullStatus = 500;
    hold();
    const map = await create();
    const first = map.addTiles3d({ url: 'a/tileset.json' });
    const idle = mock(() => {});
    map.on('idle', idle);
    // Not idle while it loads
    frames.flush();
    expect(idle).not.toHaveBeenCalled();
    release();
    await settle();
    expect(String(reported[0])).toContain('failed to load the engine from full.wasm: 500');
    expect(engineOf(map)).toBeInstanceOf(CoreFakeEngine);
    expect(first.id).toBe(-1);
    // The map goes on, idle once it failed
    frames.flush();
    expect(idle).toHaveBeenCalled();
    fullStatus = 200;
    const second = map.addTiles3d({ url: 'b/tileset.json' });
    await settle();
    expect(fullFetches()).toBeGreaterThanOrEqual(2);
    expect(
      engineOf(map)
        .args('addTiles3d')
        .map(([url]) => url),
    ).toEqual(['a/tileset.json', 'b/tileset.json']);
    expect(first.id).toBeGreaterThanOrEqual(0);
    expect(second.id).toBeGreaterThanOrEqual(0);
  });

  test('a context lost and restored while it loads: the other layers are back, the 3D Tiles layer still waits', async () => {
    hold();
    const map = await create();
    map.addLines({ data: point });
    const tiles = map.addTiles3d({ url: 'city/tileset.json' });
    const later = map.addCircles({ data: point });
    canvasOf(map).dispatch('webglcontextlost');
    expect(() => canvasOf(map).dispatch('webglcontextrestored')).not.toThrow();
    const restored = engineOf(map);
    expect(restored).toBeInstanceOf(CoreFakeEngine);
    expect(created(restored)).toEqual(['addLines', 'addCircles']);
    expect(later.id).toBeGreaterThanOrEqual(0);
    expect(tiles.id).toBe(-1);
    // Resized and drawn
    expect(restored.args('resize').length).toBe(1);
    expect(restored.args('render').length).toBeGreaterThan(0);
    release();
    await settle();
    expect(created(engineOf(map))).toEqual(['addLines', 'addTiles3d', 'addCircles']);
    expect(reported).toEqual([]);
  });

  test('a context lost when it arrives: the map moves onto it once the context is restored', async () => {
    hold();
    const map = await create();
    map.addTiles3d({ url: 'city/tileset.json' });
    canvasOf(map).dispatch('webglcontextlost');
    const core = engineOf(map);
    release();
    await settle();
    expect(engineOf(map)).toBe(core);
    canvasOf(map).dispatch('webglcontextrestored');
    expect(engineOf(map)).not.toBeInstanceOf(CoreFakeEngine);
    expect(created(engineOf(map))).toEqual(['addTiles3d']);
  });

  test("tiles3d: 'preload' compiles it once idle without running it; the first 3D Tiles layer does not download it", async () => {
    const before = fullFetches();
    const map = await create({ tiles3d: 'preload' });
    expect(fullFetches()).toBe(before);
    frames.flush();
    await settle();
    expect(fullFetches()).toBe(before + 1);
    expect(engineOf(map)).toBeInstanceOf(CoreFakeEngine);
    map.addTiles3d({ url: 'city/tileset.json' });
    await settle();
    expect(fullFetches()).toBe(before + 1);
    expect(engineOf(map)).not.toBeInstanceOf(CoreFakeEngine);
  });
});

describe('context loss', () => {
  test('buildings linked to a network above them get its new id', async () => {
    const map = await create();
    const buildings = map.addExtrusions();
    const network = map.addNetwork({ nodes: Float64Array.of(0, 0, 1, 1), edges: Uint32Array.of(0, 1) });
    buildings.linkNetwork(network, 600);
    canvasOf(map).dispatch('webglcontextlost');
    canvasOf(map).dispatch('webglcontextrestored');
    const fresh = engineOf(map);
    expect(fresh.last('linkExtrusionNetwork')).toEqual([buildings.id, network.id, 600]);
    // Never linked to an id of the old engine
    expect(fresh.args('linkExtrusionNetwork').every(([, id]) => id === network.id)).toBe(true);
  });

  test('a layer that fails to be created again is reported; the others are created', async () => {
    const map = await create();
    const first = map.addLines({ data: point });
    const failing = map.addCircles({ data: point });
    const last = map.addFills({ data: point });
    canvasOf(map).dispatch('webglcontextlost');
    // The next engine fails to create circles
    FakeEngine.failNext.add('addCircles');
    canvasOf(map).dispatch('webglcontextrestored');
    expect(String(reported[0])).toContain('addCircles failed');
    expect(failing.id).toBe(-1);
    expect(first.id).toBeGreaterThanOrEqual(0);
    expect(last.id).toBeGreaterThanOrEqual(0);
    expect(engineOf(map).args('resize').length).toBe(1);
  });

  test('a context restored after destroy brings nothing back', async () => {
    const map = await create();
    map.addLines({ data: point });
    canvasOf(map).dispatch('webglcontextlost');
    map.destroy();
    const engines = FakeEngine.instances.length;
    canvasOf(map).dispatch('webglcontextrestored');
    expect(FakeEngine.instances.length).toBe(engines);
    expect(canvasOf(map).listeners.get('webglcontextrestored')?.size ?? 0).toBe(0);
  });

  test('glyphs arriving after destroy go to the engine that does nothing', async () => {
    const map = await create();
    const engine = engineOf(map);
    const glyphs = map.glyphs;
    map.destroy();
    const calls = engine.calls.length;
    // A range arriving late uploads its page
    (glyphs as unknown as { flush(): void }).flush();
    expect(engine.calls.length).toBe(calls);
  });
});

describe('hover', () => {
  test('a mouse moving over a map nobody listens to draws nothing', async () => {
    const map = await create();
    frames.flush();
    const engine = engineOf(map);
    const renders = engine.args('render').length;
    for (let i = 0; i < 10; i += 1) {
      canvasOf(map).dispatch('pointermove', { pointerId: 1, pointerType: 'mouse', clientX: i, clientY: i });
      frames.flush();
    }
    expect(engine.args('render').length).toBe(renders);
    expect(engine.args('ground')).toEqual([]);
  });

  test('hoverfeature alone: frames for the picking, no position computed', async () => {
    const map = await create();
    map.on('hoverfeature', () => {});
    frames.flush();
    const engine = engineOf(map);
    const renders = engine.args('render').length;
    map.hover([3, 4]);
    frames.run();
    expect(engine.args('render').length).toBe(renders + 1);
    expect(engine.args('ground')).toEqual([]);
    expect(engine.last('requestPick')?.slice(0, 2)).toEqual([3, 4]);
  });

  test("the canvas's place is read once, not on every pointer move", async () => {
    const map = await create();
    map.on('hover', () => {});
    const canvas = canvasOf(map);
    canvas.left = 100;
    canvas.top = 50;
    const events: unknown[] = [];
    map.on('hover', (e) => events.push(e.point));
    for (let i = 0; i < 5; i += 1) {
      canvas.dispatch('pointermove', { pointerId: 1, pointerType: 'mouse', clientX: 110 + i, clientY: 60 });
      frames.flush();
    }
    expect(canvas.rectReads).toBe(1);
    expect(events.at(-1)).toEqual([14, 10]);
    // Read again once the map is resized
    (map as unknown as { resize(): void }).resize();
    canvas.left = 0;
    canvas.dispatch('pointermove', { pointerId: 1, pointerType: 'mouse', clientX: 110, clientY: 60 });
    frames.flush();
    expect(canvas.rectReads).toBe(2);
    expect(events.at(-1)).toEqual([110, 10]);
  });

  test('a picking the engine refuses stops the poll under way; an answer that never comes is given up', async () => {
    const map = await create();
    map.on('hoverfeature', () => {});
    const engine = engineOf(map);
    map.hover([1, 1]);
    frames.run();
    // The poll runs; the next request is refused
    engine.canPick = false;
    map.hover([2, 2]);
    frames.flush(100);
    expect(frames.pending).toBe(0);
    const polls = engine.args('pollPick').length;
    // Never answered: at most 30 polls
    engine.canPick = true;
    map.hover([3, 3]);
    frames.flush(100);
    expect(engine.args('pollPick').length - polls).toBe(30);
    expect(frames.pending).toBe(0);
  });
});
