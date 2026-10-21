/**
 * Which engine a map runs: the core one, or the one with 3D Tiles, from the start or loaded once a layer needs it
 * (the map is rebuilt on it then); and the workers decoding its 3D Tiles, started with the first engine that has them.
 */

import type { TileLoader } from '../engine/loader.ts';
import {
  type EngineClass,
  engineUrl,
  loadFullEngineLater,
  preloadFullEngine,
  startTileLoader,
} from '../engine-loader.ts';
import type { Settings } from '../settings.ts';
import type { GeoMapOptions } from '../types.ts';
import type { Engine } from '../wasm/isard.js';
import { applySettings } from './engine-settings.ts';

/** What loading the engine with 3D Tiles and its workers needs */
export type Tiles3dEngineOptions = Pick<
  GeoMapOptions,
  'wasmUrl' | 'workers' | 'workerUrl' | 'decoders' | 'pointsWasmUrl' | 'assetsUrl'
>;

/** The engine class of a map, and the engine with 3D Tiles loaded when needed */
export class MapEngines {
  /** The class of the engine: without 3D Tiles until a 3D Tiles layer is added, unless asked for from the start */
  private engineClass: EngineClass;
  /** The module of the engine with 3D Tiles once it runs the map (the decoding workers run it too) */
  private module: WebAssembly.Module | undefined;
  #upgrading: Promise<void> | undefined;
  private loader: TileLoader | undefined;
  private readonly options: Tiles3dEngineOptions;
  /** The map is destroyed: a loaded engine is not used */
  private readonly gone: () => boolean;

  /** `module`: the module of the engine with 3D Tiles if `engineClass` is that engine */
  constructor(
    engineClass: EngineClass,
    module: WebAssembly.Module | undefined,
    options: Tiles3dEngineOptions,
    gone: () => boolean,
  ) {
    this.engineClass = engineClass;
    this.module = module;
    this.options = { ...options };
    this.gone = gone;
  }

  /** The engine with 3D Tiles runs the map */
  get tiles3d(): boolean {
    return this.module !== undefined;
  }

  /** The engine with 3D Tiles is loading, for the 3D Tiles layers added on the core engine */
  get upgrading(): Promise<void> | undefined {
    return this.#upgrading;
  }

  /** A new engine drawing on `canvas`, with the settings and the decoding workers of 3D Tiles if it has them */
  create(canvas: HTMLCanvasElement, redraw: () => void, settings: Settings): Engine {
    const engine = new this.engineClass(canvas, redraw);

    applySettings(engine, settings);
    // The decoding workers of 3D Tiles, with the engine that has them (they run its module) and unless turned off
    this.loader ??= this.module && startTileLoader(this.module, this.options, engine);
    if (this.loader) engine.setTileLoader(this.loader);

    return engine;
  }

  /** Height of the geoid above the ellipsoid at a position, in metres */
  geoidHeight(lon: number, lat: number): number {
    return this.engineClass.geoidHeight(lon, lat);
  }

  /** The matrix of a custom layer's model at a position */
  customMatrix(lon: number, lat: number, altitude: number): Float32Array | undefined {
    return this.engineClass.customMatrix(lon, lat, altitude);
  }

  /** Loads the engine with 3D Tiles, once at a time: `loaded` once it is the map's class, `failed` with the error;
   * neither if the map is gone by then */
  upgrade(loaded: () => void, failed: (error: unknown) => void): void {
    this.#upgrading ??= loadFullEngineLater(engineUrl('full', this.options)).then(
      ({ module, Engine }) => {
        this.#upgrading = undefined;
        if (this.gone()) return;
        this.engineClass = Engine;
        this.module = module;
        loaded();
      },
      (error: unknown) => {
        this.#upgrading = undefined;
        if (this.gone()) return;
        failed(error);
      },
    );
  }

  /** Downloads and compiles the engine with 3D Tiles without running it, unless the map is gone or has it by then */
  preload(): void {
    preloadFullEngine(engineUrl('full', this.options), () => !this.gone() && !this.module);
  }

  /** Stops the decoding workers */
  destroy(): void {
    this.loader?.destroy();
  }
}
