/**
 * What a framework's binding (isard/react, isard/vue) does when the description of a layer changes: the least the
 * layer needs (its data, its style, its instances), or a new layer in its place when what changed is set at creation
 * (its URL, its grid, its nodes). Free of any framework.
 */

import type { Layer } from '../layers/layers.ts';
import type { LayerSpec } from '../map.ts';

type Spec = LayerSpec & Record<string, unknown>;

/** The keys `setStyle` takes, by layer type; `'all'`: every key but the data (the feature layers) */
const STYLE_KEYS: Record<LayerSpec['type'], 'all' | ReadonlySet<string>> = {
  lines: 'all',
  circles: 'all',
  fills: 'all',
  symbols: 'all',
  extrusions: 'all',
  raster: new Set(['opacity', 'saturation']),
  network: new Set(['width', 'max', 'contour', 'opacity', 'halo', 'haloOpacity', 'ramp', 'unreachedColor', 'fade']),
  tiles3d: new Set(['maxScreenSpaceError', 'opacity', 'pointSize', 'color']),
  pointcloud: new Set([
    'colorBy',
    'pointSize',
    'maxPointSize',
    'sizeMode',
    'shape',
    'edl',
    'classifications',
    'elevationRange',
    'elevationRamp',
    'intensityRange',
    'maxScreenSpaceError',
    'pointBudget',
  ]),
  models: new Set(),
  ground: new Set(),
};

/** Keys a layer takes by a method of its own, by layer type */
const OWN_SETTERS: Partial<Record<LayerSpec['type'], ReadonlySet<string>>> = {
  lines: new Set(['data']),
  circles: new Set(['data']),
  fills: new Set(['data']),
  symbols: new Set(['data']),
  extrusions: new Set(['data']),
  network: new Set(['costs', 'edgeWidths']),
  models: new Set(['instances', 'altitudeReference']),
  ground: new Set(['uniforms', 'textures']),
};

/** What to do with a layer whose description went from `prev` to `next` */
export type LayerChange =
  | { rebuild: true }
  | {
      rebuild: false;
      /** The keys changed, with their new values */
      style: Record<string, unknown>;
      own: Record<string, unknown>;
    };

/**
 * The change from one description to the next, or `undefined` when nothing changed. Values are compared by identity:
 * data, functions and arrays given anew at each render are changes (keep them in a memo)
 */
export function layerChange(prev: LayerSpec, next: LayerSpec): LayerChange | undefined {
  if (prev.type !== next.type) return { rebuild: true };
  const before = prev as Spec;
  const after = next as Spec;
  const styleKeys = STYLE_KEYS[next.type];
  const own = OWN_SETTERS[next.type];
  const change = { rebuild: false as const, style: {} as Record<string, unknown>, own: {} as Record<string, unknown> };
  let changed = false;
  for (const key of new Set([...Object.keys(before), ...Object.keys(after)])) {
    if (key === 'type' || Object.is(before[key], after[key])) continue;
    // A style or setting taken away: the layer goes back to its defaults, as made anew
    if (!(key in after)) return { rebuild: true };
    changed = true;
    if (own?.has(key)) change.own[key] = after[key];
    else if (key !== 'before' && (styleKeys === 'all' || styleKeys.has(key))) change.style[key] = after[key];
    else return { rebuild: true };
  }
  return changed ? change : undefined;
}

interface Settable {
  setStyle?(style: Record<string, unknown>): void;
  setData?(data: unknown): void;
  setCosts?(costs: unknown): void;
  setEdgeWidths?(widths: unknown): void;
  setInstances?(instances: unknown, altitudeReference?: unknown): void;
  setUniform?(name: string, value: unknown): void;
  setTexture?(name: string, texture: unknown): void;
}

/** Applies a change that keeps the layer (`rebuild: false`) */
export function applyLayerChange(
  layer: Layer,
  next: LayerSpec,
  change: Extract<LayerChange, { rebuild: false }>,
): void {
  const target = layer as unknown as Settable;
  const spec = next as Spec;
  const own = change.own;
  if ('data' in own) target.setData?.(own.data);
  if ('costs' in own) target.setCosts?.(own.costs);
  if ('edgeWidths' in own) target.setEdgeWidths?.(own.edgeWidths);
  if ('instances' in own || 'altitudeReference' in own) target.setInstances?.(spec.instances, spec.altitudeReference);
  if ('uniforms' in own) {
    for (const [name, value] of Object.entries((own.uniforms ?? {}) as Record<string, unknown>)) {
      target.setUniform?.(name, value);
    }
  }
  if ('textures' in own) {
    for (const [name, texture] of Object.entries((own.textures ?? {}) as Record<string, unknown>)) {
      target.setTexture?.(name, texture);
    }
  }
  if (Object.keys(change.style).length > 0) target.setStyle?.(change.style);
}

/**
 * The layers of a binding in their declared order: `order` (default 0) from the bottom up, layers of the same order by
 * the time they came. A layer added or made anew is moved under the first layer above it
 */
export class LayerOrder {
  private readonly entries: { layer: Layer; order: number; seq: number }[] = [];
  private seq = 0;

  /** Places a new layer, or a layer made anew in place of `old` */
  place(layer: Layer, order: number, old?: Layer): void {
    const index = old ? this.entries.findIndex((e) => e.layer === old) : -1;
    const entry = index >= 0 ? this.entries[index] : undefined;
    if (entry) {
      entry.layer = layer;
      entry.order = order;
    } else {
      this.entries.push({ layer, order, seq: this.seq++ });
    }
    this.entries.sort((a, b) => a.order - b.order || a.seq - b.seq);
    const at = this.entries.findIndex((e) => e.layer === layer);
    const above = this.entries[at + 1]?.layer;
    if (above) layer.moveBefore(above);
  }

  /** Forgets a layer removed */
  remove(layer: Layer): void {
    const index = this.entries.findIndex((e) => e.layer === layer);
    if (index >= 0) this.entries.splice(index, 1);
  }
}
