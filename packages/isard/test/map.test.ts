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
class NoWorker {
  postMessage() {}
  terminate() {}
}

const restore = setGlobals({
  ...domGlobals(frames),
  fetch: fetchMock,
  reportError: (error: unknown) => reported.push(error),
  Worker: NoWorker,
});
afterAll(restore);

const { GeoMap } = await import('../src/map.ts');
const { defaultEasing } = await import('../src/animation.ts');
const { TileLoader } = await import('../src/loader.ts');
const { geoplateformeWms, geoplateformeWmts } = await import('../src/geoplateforme.ts');
const {
  BODY_TERRAINS,
  IGN_ATTRIBUTION,
  OPENFREEMAP_ATTRIBUTION,
  TERRAIN_TILES,
  TERRAIN_TILES_ATTRIBUTION,
  TERRAIN_URL,
} = await import('../src/basemaps.ts');

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
    expect(engine.calls[0]).toEqual([
      'setBody',
      [6378137, 1 / 298.257223563, Float32Array.of(0.42, 0.66, 1, 1), -450, 8900],
    ]);
    expect(engine.last('setZoomRange')).toEqual([0, 22]);
    expect(engine.last('setMaxPitch')).toEqual([70]);
    expect(engine.last('setGlobe')).toEqual([true]);
    expect(engine.last('setSky')).toEqual([true]);
    expect(engine.last('setAtmosphere')).toEqual([undefined]);
    expect(engine.last('setTerrain')).toEqual([undefined, 1, undefined, undefined, undefined]);
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
    expect(engine.last('setTerrain')?.slice(0, 4)).toEqual([TERRAIN_TILES.url, 2, 14, false]);
    expect(map.getCamera()).toMatchObject({ center: [5, 45], zoom: 8 });
  });

  test('terrain: true takes every default; setTerrain(false) removes it', async () => {
    const { engine, map } = await create({ terrain: true });
    // The worldwide Terrain Tiles, decoded by a loader of the page, the altitudes as served
    const [url, exaggeration, maxZoom, geoid, loader] = engine.last('setTerrain') ?? [];
    expect([url, exaggeration, maxZoom, geoid]).toEqual([TERRAIN_TILES.url, 1, 14, false]);
    expect(loader).toBeInstanceOf(Function);
    // The same loader for the same tiles: the engine keeps them when the exaggeration changes
    map.setTerrain({ exaggeration: 2 });
    expect(engine.last('setTerrain')?.[4]).toBe(loader);
    map.setTerrain(false);
    expect(engine.last('setTerrain')).toEqual([undefined, 1, undefined, undefined, undefined]);
  });

  test("the IGN's terrain model by name, raised by the geoid; another encoding decoded by a loader", async () => {
    const { engine, map } = await create({ terrain: 'ign' });
    expect(engine.last('setTerrain')).toEqual([TERRAIN_URL, 1, 15, true, undefined]);
    map.setTerrain({ model: 'ign', exaggeration: 1.5, geoid: false });
    expect(engine.last('setTerrain')).toEqual([TERRAIN_URL, 1.5, 15, false, undefined]);
    map.setTerrain({ url: 'https://dem.example/{z}/{x}/{y}.png', encoding: 'mapbox' });
    const [url, , maxZoom, geoid, loader] = engine.last('setTerrain') ?? [];
    expect([url, maxZoom, geoid]).toEqual(['https://dem.example/{z}/{x}/{y}.png', 15, false]);
    expect(loader).toBeInstanceOf(Function);
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

  test('credits follow the basemap, hillshade and relief; former basemap names still work', async () => {
    const container = new FakeElement('div');
    const map = await GeoMap.create(container as unknown as HTMLElement, {
      basemap: 'ign-plan',
      workers: false,
      attributionControl: { customAttribution: 'Mine' },
    });
    maps.push(map);
    const engine = map.engine as unknown as FakeEngine;
    const credits = (element: FakeElement): string | undefined =>
      element.className === 'isard-attrib-inner' ? element.innerHTML : element.children.map(credits).find(Boolean);
    expect(credits(container)).toBe(`${IGN_ATTRIBUTION} | Mine`);
    map.setBasemap('none');
    expect(credits(container)).toBe('Mine');
    map.setTerrain(true);
    expect(credits(container)).toBe(`${TERRAIN_TILES_ATTRIBUTION} | Mine`);
    // The IGN hillshade and terrain model: the IGN credits once
    map.setTerrain('ign');
    map.setBasemap('none', 0.4);
    expect(credits(container)).toBe(`${IGN_ATTRIBUTION} | Mine`);
    // A former name throws, naming the new one
    expect(() => map.setBasemap('ortho' as never)).toThrow("is now named 'ign-ortho'");
    map.setBasemap('ign-ortho');
    expect(String(engine.last('addRaster')?.[0])).toContain('ORTHOIMAGERY.ORTHOPHOTOS');
    expect((engine.last('setGlobeColor') as number[]).map((v) => Math.round(v * 255))).toEqual([10, 59, 89]);
  });

  test("a raster's credits while it is on the map; the Géoplateforme's rasters credit the IGN", async () => {
    const container = new FakeElement('div');
    const map = await GeoMap.create(container as unknown as HTMLElement, { basemap: 'none', workers: false });
    maps.push(map);
    const credits = (element: FakeElement): string | undefined =>
      element.className === 'isard-attrib-inner' ? element.innerHTML : element.children.map(credits).find(Boolean);
    const cadastre = map.addRaster(geoplateformeWms('CADASTRALPARCELS.PARCELLAIRE_EXPRESS'));
    expect(credits(container)).toBe(IGN_ATTRIBUTION);
    const engine = map.engine as unknown as FakeEngine;
    expect(String(engine.last('addRaster')?.[0])).toContain('BBOX={bbox}&WIDTH=256&HEIGHT=256');
    // Under the basemap's credits, which come first
    map.addRaster({ url: 'https://tiles.example/{z}/{x}/{y}.png', attribution: 'Example' });
    map.setBasemap('ign-plan');
    expect(credits(container)).toBe(`${IGN_ATTRIBUTION} | Example`);
    map.setBasemap('none');
    cadastre.remove();
    expect(credits(container)).toBe('Example');
    // The same WMTS URL as the IGN's basemaps
    expect(geoplateformeWmts('ORTHOIMAGERY.ORTHOPHOTOS', { format: 'image/jpeg' }).url).toBe(
      'https://data.geopf.fr/wmts?SERVICE=WMTS&REQUEST=GetTile&VERSION=1.0.0&STYLE=normal&TILEMATRIXSET=PM' +
        '&TILEMATRIX={z}&TILEROW={y}&TILECOL={x}&LAYER=ORTHOIMAGERY.ORTHOPHOTOS&FORMAT=image/jpeg',
    );
  });

  test("OpenFreeMap's and PLAN.IGN's styles credit their sources", async () => {
    const { styleAttribution, OPENFREEMAP_STYLES, PLAN_STYLES } = await import('../src/vectortiles.ts');
    expect(styleAttribution(OPENFREEMAP_STYLES.positron)).toBe(OPENFREEMAP_ATTRIBUTION);
    expect(OPENFREEMAP_ATTRIBUTION).toContain('OpenStreetMap');
    expect(styleAttribution(PLAN_STYLES.grey)).toBe(IGN_ATTRIBUTION);
    expect(styleAttribution('https://tiles.example/style.json')).toBeUndefined();
  });

  test("the Moon: its sphere, no atmosphere, none of the Earth's defaults", async () => {
    // The map's own defaults, not those of `create`
    const container = new FakeElement('div');
    const map = await GeoMap.create(container as unknown as HTMLElement, {
      body: 'moon',
      workers: false,
      attributionControl: { customAttribution: 'NASA' },
    });
    maps.push(map);
    const engine = map.engine as unknown as FakeEngine;
    expect(engine.last('setBody')).toEqual([1737400, 0, undefined, -9200, 10800]);
    expect(engine.args('addRaster')).toEqual([]);
    expect(engine.last('setBackground')).toEqual([0, 0, 0]);
    expect((engine.last('setGlobeColor') as number[]).map((v) => Math.round(v * 255))).toEqual([0x77, 0x75, 0x6f]);
    expect(map.getCamera()).toEqual({ center: [0, 0], zoom: 2, pitch: 0, bearing: 0 });
    // The credits of the page only
    const credits = (element: FakeElement): FakeElement | undefined =>
      element.className === 'isard-attrib-inner' ? element : element.children.map(credits).find(Boolean);
    expect(credits(container)?.innerHTML).toBe('NASA');
    // NASA's LOLA model, from its image service in the Moon's Mercator, its altitudes left as served
    map.setTerrain(true);
    const [url, exaggeration, maxZoom, geoid] = engine.last('setTerrain') ?? [];
    expect([exaggeration, maxZoom, geoid]).toEqual([1, 8, false]);
    expect(url).toStartWith(
      'https://trek.nasa.gov/moon/trekarcgis/rest/services/LRO_LOLA_DEM_Global_128ppd_v04/ImageServer/exportImage?',
    );
    expect(url).toContain('format=bsq&pixelType=F32&size=256,256');
    expect(url).toEndWith('&bbox={bbox}');
    const wkt = JSON.parse(decodeURIComponent(/imageSR=([^&]+)/.exec(url as string)?.[1] ?? '')).wkt as string;
    expect(wkt).toContain('SPHEROID["S",1737400,0]');
    expect(wkt).toContain('PROJECTION["Mercator"]');
    // The braces of the WKT are escaped: only {bbox} is filled
    expect((url as string).match(/[{}]/g)).toEqual(['{', '}']);
    // A url of the page's own: its finest level 15 by default
    map.setTerrain({ url: 'https://moon.example/{z}/{x}/{y}.bil', exaggeration: 3 });
    expect(engine.last('setTerrain')).toEqual(['https://moon.example/{z}/{x}/{y}.bil', 3, 15, false, undefined]);
  });

  test('an ArcGIS image service gives raw altitudes in the Mercator of the body', async () => {
    const { engine, map } = await create({
      terrain: { url: 'https://dem.example/arcgis/rest/services/D/ImageServer/' },
    });
    expect(engine.last('setTerrain')).toEqual([
      'https://dem.example/arcgis/rest/services/D/ImageServer/exportImage?f=image&format=bsq&pixelType=F32' +
        '&size=256,256&bboxSR=3857&imageSR=3857&bbox={bbox}',
      1,
      15,
      false,
      undefined,
    ]);
    map.setTerrain({ url: 'https://dem.example/D/ImageServer?token=1' });
    expect(engine.last('setTerrain')?.[0]).toBe('https://dem.example/D/ImageServer?token=1');
  });

  test('another body: its radius and flattening, its atmosphere and color', async () => {
    const { engine } = await create({
      body: { radius: 3396190, flattening: 0.00589, atmosphere: true, color: '#c1440e' },
    });
    // The Earth's halo
    expect(engine.last('setBody')).toEqual([3396190, 0.00589, Float32Array.of(0.42, 0.66, 1, 1), -33961.9, 33961.9]);
    expect(engine.last('setGlobeColor')).toEqual([0xc1 / 255, 0x44 / 255, 0x0e / 255]);
    const { engine: sphere } = await create({ body: { radius: 1000 } });
    expect(sphere.last('setBody')).toEqual([1000, 0, undefined, -10, 10]);
    // No relief of its own: a url is needed
    await expect(create({ body: { radius: 1000 }, terrain: true })).rejects.toThrow(/needs a url/);
    const { engine: rough } = await create({ body: { radius: 1000, relief: [-50, 120] } });
    expect(rough.last('setBody')?.slice(3)).toEqual([-50, 120]);
    // A halo of its own color, its alpha the strength; none for what is not a color
    const { engine: tinted } = await create({ body: { radius: 1000, atmosphere: 'rgb(255 102 0 / 50%)' } });
    expect(tinted.last('setBody')).toEqual([1000, 0, Float32Array.of(1, 0.4, 0, 0.5), -10, 10]);
    const { engine: wrong } = await create({ body: { radius: 1000, atmosphere: 'dusty' } });
    expect(wrong.last('setBody')).toEqual([1000, 0, undefined, -10, 10]);
  });

  test("Mars: the sphere of its equatorial radius, a thin dusty halo, none of the Earth's defaults", async () => {
    const container = new FakeElement('div');
    const map = await GeoMap.create(container as unknown as HTMLElement, { body: 'mars', workers: false });
    maps.push(map);
    const engine = map.engine as unknown as FakeEngine;
    expect(engine.calls[0]).toEqual([
      'setBody',
      [3396190, 0, Float32Array.of(232 / 255, 168 / 255, 120 / 255, 0.6), -8300, 21300],
    ]);
    // Butterscotch, warmer and fainter than the Earth's blue
    const [r = 0, g = 0, b = 0, strength = 0] = (engine.last('setBody')?.[2] ?? []) as Float32Array;
    expect(r).toBeGreaterThan(g);
    expect(g).toBeGreaterThan(b);
    expect(strength).toBeLessThan(1);
    expect(engine.args('addRaster')).toEqual([]);
    expect(engine.last('setBackground')).toEqual([0, 0, 0]);
    expect((engine.last('setGlobeColor') as number[]).map((v) => Math.round(v * 255))).toEqual([0xb5, 0x71, 0x4a]);
    expect(map.getCamera()).toEqual({ center: [0, 0], zoom: 2, pitch: 0, bearing: 0 });
    // NASA's MOLA model, in the Mercator of Mars's sphere
    map.setTerrain({ exaggeration: 2 });
    const [url, exaggeration, maxZoom, geoid] = engine.last('setTerrain') ?? [];
    expect([exaggeration, maxZoom, geoid]).toEqual([2, 8, false]);
    expect(url).toStartWith(
      'https://trek.nasa.gov/mars/trekarcgis/rest/services/mola128_mola64_merge_90Nto90S_SimpleC_clon0/',
    );
    expect(decodeURIComponent(url as string)).toContain('SPHEROID[\\"S\\",3396190,0]');
  });

  test('an equirectangular raster is reprojected by a loader, one level finer in Web Mercator', async () => {
    const { engine, map } = await create();
    map.addRaster({ url: 'https://trek.example/{z}/{y}/{x}.jpg', grid: 'equirectangular', maxZoom: 8 });
    const [url, minZoom, maxZoom, , , size, loader] = engine.last('addRaster') ?? [];
    expect([url, minZoom, maxZoom, size]).toEqual(['https://trek.example/{z}/{y}/{x}.jpg', 1, 9, undefined]);
    expect(loader).toBeInstanceOf(Function);
    map.addRaster({ url: 'https://tiles.example/{z}/{x}/{y}.png' });
    expect(engine.last('addRaster')?.slice(2)).toEqual([19, 1, 1, undefined, undefined]);
  });

  test("the vector basemap's tiles are 512 pixels, or the size asked for", async () => {
    const warn = console.warn;
    // The style is not served here
    console.warn = () => undefined;
    try {
      const { engine: sharp } = await create({ basemap: 'osm-positron' });
      expect(sharp.last('addRaster')?.[5]).toBe(512);
      const { engine: light } = await create({ basemap: 'osm-positron', basemapTileSize: 256 });
      expect(light.last('addRaster')?.[5]).toBe(256);
      // The styles failed meanwhile
      await new Promise((resolve) => setTimeout(resolve, 5));
    } finally {
      console.warn = warn;
    }
  });

  test('the relief and the reprojected rasters are decoded by the workers of the map, or in the page without', async () => {
    const sent: { type: string; id: number }[] = [];
    class RecordingWorker {
      onmessage: ((event: { data: unknown }) => void) | undefined;
      postMessage(message: { type: string; id: number }) {
        sent.push(message);
        const answer =
          message.type === 'heights' ? { heights: Float32Array.of(1) } : { image: { width: 256, height: 256 } };
        queueMicrotask(() => this.onmessage?.({ data: { id: message.id, ...answer } }));
      }
      terminate() {}
    }
    const restoreWorkers = setGlobals({ Worker: RecordingWorker, OffscreenCanvas: class {} });
    const signal = new AbortController().signal;
    try {
      const { map, engine } = await create({ workers: 2, terrain: true });
      const terrain = engine.last('setTerrain')?.[4] as (...a: unknown[]) => Promise<Float32Array | null>;
      expect(await terrain(3, 4, 5, signal)).toEqual(Float32Array.of(1));
      const url = TERRAIN_TILES.url.replace('{z}', '3').replace('{x}', '4').replace('{y}', '5');
      expect(sent[0]).toMatchObject({ type: 'heights', url, encoding: 'terrarium', floor: 0 });
      map.addRaster({ url: 'https://trek.example/{z}/{y}/{x}.jpg', grid: 'equirectangular', maxZoom: 8 });
      const raster = engine.last('addRaster')?.[6] as (...a: unknown[]) => Promise<unknown>;
      expect(await raster(3, 1, 1, signal)).toEqual({ width: 256, height: 256 });
      // Planned here (level 4 around 74° N, as in the old strips: z - 1 and two levels finer there), drawn by a worker
      // from the source tiles under the Mercator tile
      const { job } = sent.at(-1) as unknown as { job: { size: number; sources: { url: string }[] } };
      expect(sent.at(-1)).toMatchObject({ type: 'reproject', job: { size: 256, n: 16 } });
      const rows = [0, 1, 2].flatMap((row) => [4, 5, 6, 7].map((col) => `https://trek.example/4/${row}/${col}.jpg`));
      expect(job.sources.map((s) => s.url)).toEqual(rows);
      // Without workers: fetched in the page
      sent.length = 0;
      fetchMock.mockClear();
      const { engine: alone } = await create({ workers: false, terrain: true });
      const local = alone.last('setTerrain')?.[4] as (...a: unknown[]) => Promise<Float32Array | null>;
      // The fetched bytes are no PNG (and the test has no createImageBitmap): a rejection, the engine retries
      await expect(local(3, 4, 5, signal)).rejects.toThrow();
      expect(fetchMock.mock.calls.map(([u]) => u)).toEqual([url]);
      expect(sent).toEqual([]);
    } finally {
      restoreWorkers();
    }
  });

  test('an unknown basemap throws, before anything is created or removed', async () => {
    await expect(create({ basemap: 'nope' as never })).rejects.toThrow("isard: unknown basemap 'nope'");
    const { map, engine } = await create({ basemap: 'ign-plan' });
    expect(() => map.setBasemap('plan-ortho' as never)).toThrow("isard: unknown basemap 'plan-ortho'");
    expect(engine.args('removeLayer')).toEqual([]);
  });

  test('a PNG encoding of the relief takes a tile URL: an image service or a {bbox} throws', async () => {
    const { map } = await create();
    expect(() => map.setTerrain({ url: 'https://dem.example/ImageServer', encoding: 'terrarium' })).toThrow(
      "isard: the 'terrarium' terrain encoding takes a tile URL with {z}, {x} and {y}",
    );
    expect(() => map.setTerrain({ url: 'https://dem.example/wms?BBOX={bbox}', encoding: 'mapbox' })).toThrow(
      "the 'mapbox' terrain encoding",
    );
    expect(() => map.setTerrain({ url: 'https://dem.example/wms?BBOX={bbox}' })).not.toThrow();
  });

  test("NASA's relief of the Moon and Mars is credited; a relief of the application's is not", async () => {
    const container = new FakeElement('div');
    const map = await GeoMap.create(container as unknown as HTMLElement, {
      body: 'moon',
      terrain: true,
      workers: false,
    });
    maps.push(map);
    const credits = (element: FakeElement): string | undefined =>
      element.className === 'isard-attrib-inner' ? element.innerHTML : element.children.map(credits).find(Boolean);
    expect(credits(container)).toBe(BODY_TERRAINS.moon.attribution);
    expect(credits(container)).toContain('LOLA terrain model, NASA/GSFC/MIT');
    map.setTerrain({ url: 'https://dem.example/ImageServer' });
    expect(credits(container) ?? '').toBe('');
    const mars = await create({ body: 'mars', terrain: true, attributionControl: true });
    expect(credits(mars.container)).toContain('MOLA terrain model, NASA/JPL/GSFC');
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

describe('layers', () => {
  test("a raster's style: either value alone keeps the other", async () => {
    const { map, engine } = await create();
    const raster = map.addRaster({ url: 'https://tiles.example/{z}/{x}/{y}.png', opacity: 0.8, saturation: 0.5 });
    raster.setStyle({ opacity: 0.3 });
    expect(engine.last('setRasterStyle')).toEqual([raster.id, 0.3, 0.5]);
    raster.setStyle({ saturation: 0 });
    expect(engine.last('setRasterStyle')).toEqual([raster.id, 0.3, 0]);
  });

  test('map.add: a layer by its type and options, the same as its add… method; an unknown type throws', async () => {
    const { map, engine } = await create();
    const fills = map.add({ type: 'fills', data: [], color: '#ff0000' });
    const lines = map.add({ type: 'lines', data: [], before: fills });
    expect(engine.args('addFills').length + engine.args('addLines').length).toBe(2);
    const order = (map as unknown as { layers: unknown[] }).layers;
    expect(order.indexOf(lines)).toBeLessThan(order.indexOf(fills));
    expect(() => map.add({ type: 'nothing' } as never)).toThrow('no layer of type nothing');
  });

  test("the grouped methods: map.camera, map.query, map.effects are the map's own", async () => {
    const { map, engine } = await create();
    map.camera.set({ zoom: 9, pitch: 30 });
    expect(engine.last('setCamera')?.slice(2)).toEqual([9, 30, 0]);
    expect(map.camera.get()).toEqual(map.getCamera());
    map.effects.viewshed(undefined);
    expect(engine.args('clearViewshed').length).toBe(1);
    expect(map.query.elevationAt([2.35, 48.85])).toBe(map.elevationAt([2.35, 48.85]));
  });

  test('a preset under the options given', async () => {
    const { map, engine } = await create({ preset: 'ign-3d', pitch: 20 });
    expect(map.getCamera().pitch).toBe(20);
    // The preset's relief (the IGN's), its basemap overridden by the test's 'none'
    expect(String(engine.last('setTerrain')?.[0])).toContain('geopf');
  });

  test('a removed layer is removed once', async () => {
    const { map, engine } = await create();
    const line = map.addLines({ data: [] });
    line.remove();
    line.remove();
    expect(engine.args('removeLayer').length).toBe(1);
  });

  test('vector tiles: a removed labels layer is forgotten; destroy removes the layers still there', async () => {
    const { map, engine } = await create();
    const internals = map as unknown as { onTop: Set<unknown>; vectorSets: Set<unknown> };
    const tiles = map.vectorTiles({
      style: { version: 8, sources: { v: { type: 'vector', tiles: ['https://t/{z}/{x}/{y}'] } }, layers: [] },
    });
    const raster = tiles.raster();
    const labels = tiles.labels();
    const other = tiles.raster();
    expect(internals.onTop.has(labels)).toBe(true);
    labels.remove();
    expect(internals.onTop.size).toBe(0);
    other.remove();
    const removed = engine.args('removeLayer').length;
    const id = raster.id;
    tiles.destroy();
    // Only the raster still on the map
    expect(engine.args('removeLayer').slice(removed)).toEqual([[id]]);
    expect(internals.vectorSets.size).toBe(0);
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
    expect(fresh.calls[0]).toEqual(['setBody', [1737400, 0, undefined, -9200, 10800]]);
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
