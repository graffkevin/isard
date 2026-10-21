/** The viewshed's observers given to the engine, in the arrays it reads. */

import { rgb } from '../settings.ts';
import type { ViewshedOptions } from '../types.ts';
import type { Engine } from '../wasm/isard.js';

/** The observers of a viewshed option: none, one, or several */
const observersOf = (options: ViewshedOptions | readonly ViewshedOptions[] | undefined): readonly ViewshedOptions[] => {
  if (!options) return [];

  return 'position' in options ? [options] : options;
};

/** Sets the viewshed of the engine, or clears it without observers; true if one is set (a frame to draw) */
export const applyViewshed = (
  engine: Engine,
  options: ViewshedOptions | readonly ViewshedOptions[] | undefined,
): boolean => {
  const observers = observersOf(options);
  const [first] = observers;

  if (!first) {
    engine.clearViewshed();

    return false;
  }
  engine.setViewshed(
    new Float64Array(
      observers.flatMap((o) => [
        o.position[0],
        o.position[1],
        o.height ?? 1.7,
        o.range ?? 1000,
        ...(o.direction === undefined
          ? [Number.NaN, 0, 0, 0]
          : [o.direction, o.tilt ?? 0, o.fov ?? 60, o.verticalFov ?? 40]),
      ]),
    ),
    new Float32Array(rgb(first.visibleColor ?? '#2f9e44')),
    new Float32Array(rgb(first.hiddenColor ?? '#e03131')),
    first.opacity ?? 0.7,
    new Float32Array(first.coneColor ? rgb(first.coneColor) : []),
  );

  return true;
};
