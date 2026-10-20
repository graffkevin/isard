/**
 * The two builds of the engine, downloaded, compiled and instantiated once per page; the decoding workers of 3D Tiles
 * that run the full one; the engine of a destroyed map.
 */
import { DEFAULT_DECODERS } from './decoders.ts';
import { TileLoader } from './loader.ts';
import type { GeoMapOptions } from './types.ts';
import { defaultWorkerCount } from './vectortiles.ts';
import type { Camera, Engine } from './wasm/isard.js';

/** The class of an engine build: its constructor and its static `customMatrix` (custom layers) */
export type EngineClass = (new (
  canvas: HTMLCanvasElement,
  redraw: () => void,
) => Engine) &
  Pick<typeof Engine, 'customMatrix' | 'geoidHeight'>;

/** An engine loaded: its compiled module and its class */
export interface Bindings {
  module: WebAssembly.Module;
  Engine: EngineClass;
}

/** `core`: the engine without 3D Tiles; `full`: with them */
export type EngineKind = 'core' | 'full';

/** The two builds of the engine, each compiled once per page: `core` without 3D Tiles (a third smaller), loaded by
 * every map; `full` with them, loaded for the first 3D Tiles layer, its module shared with the decoding workers. The
 * core engine has the same API but the 3D Tiles methods: its class is typed as the full one. */
const engines: { core?: Promise<Bindings>; full?: Promise<Bindings> } = {};
/** The modules compiled (downloaded and compiled off the main thread, not instantiated): `tiles3d: 'preload'`
 * compiles the full engine ahead, its first 3D Tiles layer then only instantiates it */
const modules: { core?: Promise<WebAssembly.Module>; full?: Promise<WebAssembly.Module> } = {};

/** @internal Forgets the engines loaded (tests) */
export function forgetEngines(): void {
  for (const kind of ['core', 'full'] as const) {
    delete engines[kind];
    delete modules[kind];
  }
}

/** Where a build of the engine is loaded from: the option, or next to the module */
export function engineUrl(kind: EngineKind, options: Pick<GeoMapOptions, 'wasmUrl' | 'coreWasmUrl'>): string | URL {
  return kind === 'full'
    ? (options.wasmUrl ?? new URL('./isard_bg.wasm', import.meta.url))
    : (options.coreWasmUrl ?? new URL('./isard_core_bg.wasm', import.meta.url));
}

/** Downloads and compiles a build (once per page, unless it failed: a later call tries again) */
function compileEngine(kind: EngineKind, url: string | URL): Promise<WebAssembly.Module> {
  const compiled = modules[kind];
  if (compiled) return compiled;
  const compiling = compile(url).catch((error: unknown) => {
    // A later call tries again (a network error, a wrong URL fixed)
    delete modules[kind];
    throw error;
  });
  modules[kind] = compiling;
  return compiling;
}

/** A build compiled and instantiated with its bindings (once per page, unless it failed) */
export function loadEngine(kind: EngineKind, url: string | URL): Promise<Bindings> {
  const loaded = engines[kind];
  if (loaded) return loaded;
  const loading = (async () => {
    // The bindings by dynamic import: an application bundles the ones it loads only
    const [module, bindings] = await Promise.all([
      compileEngine(kind, url),
      kind === 'core' ? import('./wasm/isard_core.js') : import('./wasm/isard.js'),
    ]);
    await bindings.default({ module_or_path: module });
    return { module, Engine: bindings.Engine as unknown as EngineClass };
  })().catch((error: unknown) => {
    delete engines[kind];
    throw error;
  });
  engines[kind] = loading;
  return loading;
}

/** The full engine for a map on the core one, resolved in a task of its own, not in the one that instantiated it:
 * the map rebuilt on it there, two shorter tasks */
export function loadFullEngineLater(url: string | URL): Promise<Bindings> {
  return loadEngine('full', url).then(
    (bindings) => new Promise<Bindings>((resolve) => setTimeout(() => resolve(bindings), 0)),
  );
}

/** Downloads and compiles the full engine when the page is idle, without running it; a failure is left to the first
 * 3D Tiles layer, which tries again */
export function preloadFullEngine(url: string | URL, wanted: () => boolean): void {
  const start = () => {
    if (wanted()) compileEngine('full', url).catch(() => undefined);
  };
  if (typeof requestIdleCallback === 'function') requestIdleCallback(start, { timeout: 2000 });
  else setTimeout(start, 0);
}

async function compile(url: string | URL): Promise<WebAssembly.Module> {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`isard: failed to load the engine from ${url}: ${response.status}`);
  try {
    return await WebAssembly.compileStreaming(response.clone());
  } catch {
    // Servers that do not send application/wasm
    return WebAssembly.compile(await response.arrayBuffer());
  }
}

/** Where the workers load the point cloud module from (`isard_points_bg.wasm`, the LAZ decoder): the option, or next
 * to the module; absolute, the workers being elsewhere */
export function pointsUrl(options: Pick<GeoMapOptions, 'pointsWasmUrl'>): string {
  const url = options.pointsWasmUrl ?? new URL('./isard_points_bg.wasm', import.meta.url);
  const base = typeof document === 'undefined' ? undefined : document.baseURI;
  return base ? new URL(url, base).href : String(url);
}

/** The decoding workers of 3D Tiles and point clouds, running the full engine's module, unless turned off (`workers:
 * false` or 0, or no workers in this environment) */
export function startTileLoader(
  module: WebAssembly.Module,
  options: Pick<GeoMapOptions, 'workers' | 'workerUrl' | 'decoders' | 'pointsWasmUrl'>,
  engine: Engine,
): TileLoader | undefined {
  const { workers: count, workerUrl, decoders } = options;
  const workers = count ?? defaultWorkerCount();
  if (workers === false || workers <= 0 || typeof Worker === 'undefined') return undefined;
  return new TileLoader(
    module,
    workerUrl ?? new URL('./decoder.worker.js', import.meta.url),
    workers,
    { ...DEFAULT_DECODERS, ...decoders },
    engine.textureFormats(),
    pointsUrl(options),
  );
}

/** The engine of a destroyed map: every method a no-op returning undefined, but the camera (the last one) and the
 * bounds, which late callers read */
export function destroyedEngine(camera: Camera): Engine {
  return new Proxy(
    {},
    {
      get: (_, name) => {
        if (name === 'camera') return () => camera;
        if (name === 'bounds') return () => Float64Array.of(-180, -85, 180, 85);
        return () => undefined;
      },
    },
  ) as Engine;
}
