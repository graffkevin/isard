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

const { GeoMap } = await import('../src/map.ts');
const { forgetEngines } = await import('../src/engine-loader.ts');

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
    .filter(([name]) => /^add(Lines|Circles|Fills|Tiles3d|Extrusions|Network|Raster|Models|CustomLayer)$/.test(name))
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

describe('models, on the engine with 3D Tiles', () => {
  test('the first model moves the map onto it; instances set meanwhile are kept', async () => {
    hold();
    const map = await create();
    const below = map.addLines({ data: point });
    const models = map.addModels({
      url: 'turbine.glb',
      instances: [{ lonLat: [3, 44], heading: 90 }],
      altitudeReference: 'sea',
    });
    expect(models.id).toBe(-1);
    expect(engineOf(map).args('addModels')).toEqual([]);
    models.setInstances([{ lonLat: [3, 44] }, { lonLat: [3.1, 44.1], altitude: 20, scale: 2 }]);
    release();
    await settle();
    const full = engineOf(map);
    expect(full).not.toBeInstanceOf(CoreFakeEngine);
    expect(created(full)).toEqual(['addLines', 'addModels']);
    expect(below.id).toBeGreaterThanOrEqual(0);
    const [url, numbers, ground] = full.last('addModels') ?? [];
    expect(url).toBe('turbine.glb');
    expect([...(numbers as Float64Array)]).toEqual([3, 44, 0, 0, 0, 0, 1, 3.1, 44.1, 20, 0, 0, 0, 2]);
    expect(ground).toBe(false);
    // Changed once there: sent at once, the altitude reference kept
    models.setInstances([{ lonLat: [1, 2], pitch: 5, roll: 6 }]);
    expect(full.last('setModelInstances')?.slice(1)).toEqual([Float64Array.of(1, 2, 0, 0, 5, 6, 1), false]);
    expect(reported).toEqual([]);
  });

  test("a model's bytes go by a blob URL, revoked once removed; a model needs a url or data", async () => {
    const revoked: string[] = [];
    const revoke = URL.revokeObjectURL;
    URL.revokeObjectURL = (url: string) => revoked.push(url);
    try {
      const map = await create({ tiles3d: true });
      const models = map.addModels({ data: new Uint8Array([1, 2, 3]), instances: [] });
      const [url] = engineOf(map).last('addModels') ?? [];
      expect(String(url)).toStartWith('blob:');
      // Kept for a context loss
      canvasOf(map).dispatch('webglcontextlost');
      canvasOf(map).dispatch('webglcontextrestored');
      expect(engineOf(map).last('addModels')?.[0]).toBe(url);
      expect(revoked).toEqual([]);
      models.remove();
      expect(revoked).toEqual([String(url)]);
      expect(() => map.addModels({ instances: [] })).toThrow('a model needs a url or data');
    } finally {
      URL.revokeObjectURL = revoke;
    }
  });

  test('a model and a context loss: created again in the new engine', async () => {
    const map = await create({ tiles3d: true });
    map.addModels({ url: 'a.glb', instances: [{ lonLat: [0, 0] }] });
    canvasOf(map).dispatch('webglcontextlost');
    canvasOf(map).dispatch('webglcontextrestored');
    expect(created(engineOf(map))).toEqual(['addModels']);
  });
});

describe('custom layers', () => {
  /** A custom layer recording its calls */
  function custom(renderingMode?: '2d' | '3d') {
    const calls: string[] = [];
    const frames: { matrix: Float32Array; camera: unknown }[] = [];
    const layer = {
      ...(renderingMode ? { renderingMode } : {}),
      onAdd: mock((_gl: WebGL2RenderingContext, _map: unknown) => calls.push('add')),
      render: mock(
        (_gl: WebGL2RenderingContext, frame: { matrixAt(p: [number, number]): Float32Array; camera: unknown }) => {
          calls.push('render');
          frames.push({ matrix: frame.matrixAt([2, 48]), camera: frame.camera });
        },
      ),
      onRemove: mock((_gl: WebGL2RenderingContext, _map: unknown) => calls.push('remove')),
    };
    return { layer, calls, frames };
  }
  /** Draws a frame of the engine, calling the custom layers it holds as the engine does */
  const draw = (engine: FakeEngine) => {
    for (const [callback] of engine.args('addCustomLayer')) (callback as () => void)();
  };

  test('added, drawn with the matrices of the frame, removed', async () => {
    const map = await create();
    const { layer, calls, frames } = custom('3d');
    const handle = map.addCustomLayer(layer);
    const engine = engineOf(map);
    expect(layer.onAdd).toHaveBeenCalledTimes(1);
    expect(layer.onAdd.mock.calls[0]?.[1]).toBe(map);
    expect(engine.last('addCustomLayer')?.[1]).toBe(true);
    expect(handle.id).toBeGreaterThanOrEqual(0);
    expect(handle.implementation).toBe(layer);
    FakeEngine.matrix = Float32Array.of(1, 2, 3);
    try {
      draw(engine);
    } finally {
      FakeEngine.matrix = undefined;
    }
    expect(frames[0]?.matrix).toEqual(Float32Array.of(1, 2, 3));
    handle.remove();
    expect(engine.last('removeLayer')).toEqual([expect.any(Number)]);
    expect(calls).toEqual(['add', 'render', 'remove']);
    // Removed twice, or with the map destroyed: onRemove once
    handle.remove();
    map.destroy();
    expect(layer.onRemove).toHaveBeenCalledTimes(1);
  });

  test("'2d' by default; an error of the page is reported, the frame goes on; matrices only while drawing", async () => {
    const map = await create();
    const { layer } = custom();
    map.addCustomLayer(layer);
    const failing = map.addCustomLayer({
      render: () => {
        throw new Error('page bug');
      },
    });
    const engine = engineOf(map);
    expect(engine.args('addCustomLayer').map(([, depth]) => depth)).toEqual([false, false]);
    // Outside the engine's frame: no matrix
    expect(() => draw(engine)).not.toThrow();
    expect(String(reported[0])).toContain('only available during render');
    expect(String(reported[1])).toContain('page bug');
    expect(failing.id).toBeGreaterThanOrEqual(0);
  });

  test('the camera of the frame while the engine draws, without asking the busy engine', async () => {
    const map = await create();
    map.setCamera({ center: [5, 45], zoom: 12 });
    const { layer, frames: seen } = custom();
    map.addCustomLayer(layer);
    const engine = engineOf(map);
    let asked = 0;
    engine.onRender = () => {
      const before = engine.args('camera').length;
      FakeEngine.matrix = new Float32Array(16);
      draw(engine);
      FakeEngine.matrix = undefined;
      asked += engine.args('camera').length - before;
    };
    map.requestRender();
    frames.flush();
    engine.onRender = undefined;
    expect(seen.at(-1)?.camera).toMatchObject({ center: [5, 45], zoom: 12 });
    expect(asked).toBe(0);
    // Outside the frame, the engine's
    expect(map.getCamera()).toMatchObject({ center: [5, 45], zoom: 12 });
  });

  test('a context lost and restored: onAdd again in the new engine, in its place', async () => {
    const map = await create();
    const below = map.addLines({ data: point });
    const { layer, calls } = custom('3d');
    map.addCustomLayer(layer, { before: below });
    canvasOf(map).dispatch('webglcontextlost');
    canvasOf(map).dispatch('webglcontextrestored');
    // In its place, under the lines
    expect(created(engineOf(map))).toEqual(['addCustomLayer', 'addLines']);
    // Its objects were lost with the context: made again, not freed
    expect(calls).toEqual(['add', 'add']);
  });

  test('the map moving onto the engine with 3D Tiles keeps its objects (the same context)', async () => {
    hold();
    const map = await create();
    const { layer, calls } = custom('3d');
    map.addCustomLayer(layer);
    map.addTiles3d({ url: 'city/tileset.json' });
    release();
    await settle();
    const full = engineOf(map);
    expect(full).not.toBeInstanceOf(CoreFakeEngine);
    expect(full.args('addCustomLayer').length).toBe(1);
    expect(calls).toEqual(['add']);
  });
});
