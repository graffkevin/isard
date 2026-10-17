/**
 * Test doubles: the WebAssembly engine module (mocked once for the whole test run, configured per test through
 * `wasm`), a minimal DOM and a manual frame scheduler. Import this file before importing (dynamically) a module that
 * imports `../src/wasm/isard.js`.
 */
import { mock } from 'bun:test';
import type { Camera, PickResult } from '../src/wasm/isard.js';

type Call = [name: string, args: unknown[]];

/** A stand-in for the engine: records every call; methods not defined here return a new id (`add…`) or undefined */
export class FakeEngine {
  static instances: FakeEngine[] = [];
  calls: Call[] = [];
  cam: Camera = { center: [0, 0], zoom: 0, pitch: 0, bearing: 0 };
  /** What `render` returns: true while tiles load */
  loading = false;
  /** What `requestPick` returns */
  canPick = true;
  /** Answers of `pollPick`, one per call (undefined: not ready yet) */
  polls: (PickResult | undefined)[] = [];
  boundsValue = Float64Array.of(1, 2, 3, 4);
  freed = false;
  private nextId = 1;

  constructor(
    readonly canvas: unknown,
    readonly onRequest: () => void,
  ) {
    FakeEngine.instances.push(this);
    const own: Record<string, (...args: never[]) => unknown> = {
      camera: () => ({ ...this.cam, center: [...this.cam.center] }),
      setCamera: (lon: number, lat: number, zoom: number, pitch: number, bearing: number) => {
        // As the engine's camera clamp: longitude and bearing wrapped to [-180, 180)
        const wrap = (degrees: number) =>
          degrees >= -180 && degrees < 180 ? degrees : ((((degrees + 540) % 360) + 360) % 360) - 180;
        this.cam = { center: [wrap(lon), lat], zoom, pitch, bearing: wrap(bearing) };
      },
      bounds: () => this.boundsValue,
      render: () => this.loading,
      requestPick: () => this.canPick,
      pollPick: () => this.polls.shift(),
      ground: (x: number, y: number) => [x / 10, y / 10],
      textureFormats: () => ({ astc: false, bptc: false, etc: false, s3tc: false }),
      free: () => {
        this.freed = true;
      },
    };
    // biome-ignore lint/correctness/noConstructorReturn: a recording proxy stands for the engine
    return new Proxy(this, {
      get: (target, name, receiver) => {
        if (typeof name === 'symbol' || name in target) return Reflect.get(target, name, receiver);
        return (...args: unknown[]) => {
          target.calls.push([name, args]);
          const f = own[name];
          if (f) return (f as (...a: unknown[]) => unknown)(...args);
          return name.startsWith('add') ? target.nextId++ : undefined;
        };
      },
    });
  }

  /** The arguments of each call to `name` */
  args(name: string): unknown[][] {
    return this.calls.filter(([n]) => n === name).map(([, a]) => a);
  }

  /** The arguments of the last call to `name` */
  last(name: string): unknown[] | undefined {
    return this.args(name).at(-1);
  }
}

/** The module's free functions and init, configurable per test */
export const wasm = {
  init: mock(async (_input?: unknown) => ({})),
  decodeTile: mock(
    (_bytes: Uint8Array, _transform: Float64Array, _yUp: boolean, _draco: unknown) =>
      ({ anchor: [0, 0, 0], primitives: [] }) as unknown,
  ),
  dracoRequests: mock((_bytes: Uint8Array): unknown[] => []),
};

const bindings = () => ({
  default: (input?: unknown) => wasm.init(input),
  Engine: FakeEngine,
  decodeTile: (...args: Parameters<typeof wasm.decodeTile>) => wasm.decodeTile(...args),
  dracoRequests: (bytes: Uint8Array) => wasm.dracoRequests(bytes),
});
// The engine without 3D Tiles (loaded by default) and the full one: the same fake
mock.module('../src/wasm/isard.js', bindings);
mock.module('../src/wasm/isard_core.js', bindings);

// ── DOM ──

type Handler = (event: { type: string } & Record<string, unknown>) => void;

export class FakeElement {
  readonly style: Record<string, string> = {};
  readonly dataset: Record<string, string> = {};
  readonly attributes = new Map<string, string>();
  readonly listeners = new Map<string, Set<Handler>>();
  children: FakeElement[] = [];
  parent: FakeElement | undefined;
  className = '';
  textContent = '';
  innerHTML = '';
  tabIndex = -1;
  clientWidth = 800;
  clientHeight = 600;
  width = 0;
  height = 0;
  readonly classList = {
    add: (...names: string[]) => this.setClasses([...this.classes(), ...names]),
    remove: (...names: string[]) => this.setClasses(this.classes().filter((c) => !names.includes(c))),
    contains: (name: string) => this.classes().includes(name),
    toggle: (name: string, force?: boolean) => {
      const on = force ?? !this.classList.contains(name);
      if (on) this.classList.add(name);
      else this.classList.remove(name);
      return on;
    },
  };

  constructor(readonly tagName: string) {}

  private classes(): string[] {
    return this.className.split(/\s+/).filter(Boolean);
  }

  private setClasses(names: string[]): void {
    this.className = [...new Set(names)].join(' ');
  }

  append(...children: FakeElement[]): void {
    for (const child of children) {
      child.remove();
      child.parent = this;
      this.children.push(child);
    }
  }

  prepend(...children: FakeElement[]): void {
    for (const child of children.reverse()) {
      child.remove();
      child.parent = this;
      this.children.unshift(child);
    }
  }

  remove(): void {
    if (!this.parent) return;
    this.parent.children = this.parent.children.filter((c) => c !== this);
    this.parent = undefined;
  }

  setAttribute(name: string, value: string): void {
    this.attributes.set(name, value);
  }

  getAttribute(name: string): string | null {
    return this.attributes.get(name) ?? null;
  }

  addEventListener(type: string, handler: Handler): void {
    const set = this.listeners.get(type) ?? new Set();
    this.listeners.set(type, set);
    set.add(handler);
  }

  removeEventListener(type: string, handler: Handler): void {
    this.listeners.get(type)?.delete(handler);
  }

  dispatch(type: string, fields: Record<string, unknown> = {}): void {
    for (const handler of [...(this.listeners.get(type) ?? [])]) handler({ type, preventDefault() {}, ...fields });
  }

  querySelectorAll(): FakeElement[] {
    return [];
  }

  getContext(): null {
    return null;
  }

  get ownerDocument(): FakeDocument {
    return fakeDocument;
  }
}

class FakeDocument {
  head = new FakeElement('head');
  createElement(tag: string): FakeElement {
    return new FakeElement(tag);
  }
}

const fakeDocument = new FakeDocument();

// ── Frames ──

/** requestAnimationFrame by hand: `frames.run(now)` calls the callbacks queued so far */
export class Frames {
  private queue = new Map<number, (now: number) => void>();
  private next = 1;
  readonly request = (callback: (now: number) => void): number => {
    const id = this.next++;
    this.queue.set(id, callback);
    return id;
  };
  readonly cancel = (id: number): void => {
    this.queue.delete(id);
  };
  get pending(): number {
    return this.queue.size;
  }
  /** Runs the callbacks queued before this call (those they queue wait for the next run) */
  run(now = performance.now()): void {
    const callbacks = [...this.queue.values()];
    this.queue.clear();
    for (const callback of callbacks) callback(now);
  }
  /** Runs frames until none is asked for (at most `max`) */
  flush(max = 50, now = performance.now()): void {
    for (let i = 0; i < max && this.queue.size > 0; i += 1) this.run(now);
  }
}

/** Replaces globals for the time of a test file; returns the function that puts them back */
export function setGlobals(values: Record<string, unknown>): () => void {
  const g = globalThis as Record<string, unknown>;
  const saved = Object.keys(values).map((key) => [key, Object.getOwnPropertyDescriptor(g, key)] as const);
  for (const [key, value] of Object.entries(values)) {
    Object.defineProperty(g, key, { value, configurable: true, writable: true });
  }
  return () => {
    for (const [key, descriptor] of saved) {
      if (descriptor) Object.defineProperty(g, key, descriptor);
      else delete g[key];
    }
  };
}

/** Records what ResizeObserver watches */
export class FakeResizeObserver {
  static instances: FakeResizeObserver[] = [];
  observed: unknown[] = [];
  disconnected = false;
  constructor(readonly callback: () => void) {
    FakeResizeObserver.instances.push(this);
  }
  observe(target: unknown): void {
    this.observed.push(target);
  }
  disconnect(): void {
    this.disconnected = true;
  }
}

/** The globals a map needs, with a frame scheduler */
export function domGlobals(frames: Frames): Record<string, unknown> {
  return {
    document: fakeDocument,
    window: { devicePixelRatio: 2 },
    getComputedStyle: () => ({ position: 'static' }),
    ResizeObserver: FakeResizeObserver,
    requestAnimationFrame: frames.request,
    cancelAnimationFrame: frames.cancel,
  };
}

/** The smallest valid WebAssembly module (magic number, version 1) */
export const EMPTY_WASM = Uint8Array.of(0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00);
