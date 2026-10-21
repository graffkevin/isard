/**
 * What a framework's binding (isard/react, isard/vue) does when the description of a layer changes: the least the
 * layer needs (its data, its style, its instances), or a new layer in its place when what changed is set at creation
 * (its URL, its grid, its nodes). Free of any framework.
 */

import type { Layer } from '../layers/layers.ts';
import type { GeoMap, LayerSpec } from '../map.ts';

/** A layer as a binding describes it: the options of `map.add`, and the values of its features or nodes
 * (`setValues`: what colors them by a ramp) */
export type BoundLayerSpec<P = Record<string, unknown>> = LayerSpec<P> & { values?: Float32Array };

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
  lines: new Set(['data', 'filter', 'values']),
  circles: new Set(['data', 'filter', 'values']),
  fills: new Set(['data', 'filter', 'values']),
  symbols: new Set(['data', 'filter']),
  extrusions: new Set(['data', 'filter', 'values']),
  network: new Set(['costs', 'edgeWidths', 'values']),
  models: new Set(['instances', 'altitudeReference']),
  ground: new Set(['uniforms', 'textures']),
};

/** The keys of a description and their values */
const fields = (spec: BoundLayerSpec): Map<string, unknown> => new Map(Object.entries(spec));

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
export const layerChange = (prev: BoundLayerSpec, next: BoundLayerSpec): LayerChange | undefined => {
  if (prev.type !== next.type) return { rebuild: true };

  const before = fields(prev);
  const after = fields(next);
  const styleKeys = STYLE_KEYS[next.type];
  const own = OWN_SETTERS[next.type];
  const change: Extract<LayerChange, { rebuild: false }> = { rebuild: false, style: {}, own: {} };
  const keys = [...new Set([...before.keys(), ...after.keys()])].filter(
    (key) => key !== 'type' && !Object.is(before.get(key), after.get(key)),
  );

  if (keys.length === 0) return undefined;

  const isStyle = (key: string) => key !== 'before' && (styleKeys === 'all' || styleKeys.has(key));

  // A style or setting taken away: the layer goes back to its defaults, as made anew
  if (keys.some((key) => !after.has(key) || !(own?.has(key) || isStyle(key)))) return { rebuild: true };
  keys.forEach((key) => {
    if (own?.has(key)) change.own[key] = after.get(key);
    else change.style[key] = after.get(key);
  });

  return change;
};

/** The methods a change calls on a layer, those of its type */
type Setter =
  | 'setStyle'
  | 'setData'
  | 'setFilter'
  | 'setValues'
  | 'setCosts'
  | 'setEdgeWidths'
  | 'setInstances'
  | 'setUniform'
  | 'setTexture';

/** Calls a setter of the layer, if its type has it */
const set = (layer: Layer, setter: Setter, ...args: unknown[]): void => {
  const method: unknown = Reflect.get(layer, setter);

  if (typeof method === 'function') method.apply(layer, args);
};

/** Applies a change that keeps the layer (`rebuild: false`) */
export const applyLayerChange = (
  layer: Layer,
  next: BoundLayerSpec,
  change: Extract<LayerChange, { rebuild: false }>,
): void => {
  const spec = fields(next);
  const own = change.own;

  if ('data' in own) set(layer, 'setData', own.data);
  if ('filter' in own) set(layer, 'setFilter', own.filter);
  if ('values' in own) set(layer, 'setValues', own.values);
  if ('costs' in own) set(layer, 'setCosts', own.costs);
  if ('edgeWidths' in own) set(layer, 'setEdgeWidths', own.edgeWidths);
  if ('instances' in own || 'altitudeReference' in own) {
    set(layer, 'setInstances', spec.get('instances'), spec.get('altitudeReference'));
  }
  if ('uniforms' in own) {
    Object.entries(own.uniforms ?? {}).forEach(([name, value]) => {
      set(layer, 'setUniform', name, value);
    });
  }
  if ('textures' in own) {
    Object.entries(own.textures ?? {}).forEach(([name, texture]) => {
      set(layer, 'setTexture', name, texture);
    });
  }
  if (Object.keys(change.style).length > 0) set(layer, 'setStyle', change.style);
};

/** Adds a layer from its description, its values set */
export const addBoundLayer = (map: GeoMap, { values, ...spec }: BoundLayerSpec): Layer => {
  const layer = map.add(spec as LayerSpec);

  if (values) set(layer, 'setValues', values);

  return layer;
};

/** A layer made anew from its description in the place of `old`, its values set */
export const rebuildBoundLayer = (map: GeoMap, old: Layer, { values, ...spec }: BoundLayerSpec): Layer => {
  const layer = map.rebuildLayer(old, spec as LayerSpec);

  if (values) set(layer, 'setValues', values);

  return layer;
};

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
