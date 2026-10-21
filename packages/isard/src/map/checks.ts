/**
 * What the map checks of what the application gives it before the engine sees it (a basemap's name, a network's
 * arrays), and how it reports an error it does not throw.
 */

import { BASEMAPS, RENAMED_BASEMAPS } from '../basemap/basemaps.ts';
import { VECTOR_BASEMAPS } from '../basemap/vectortiles.ts';
import type { Basemap, BasemapName } from '../types.ts';

/** Reports an error without throwing it here: the console shows it, as an uncaught one */
export const report = (error: unknown): void => {
  if (typeof reportError === 'function') reportError(error);
  else
    queueMicrotask(() => {
      throw error;
    });
};

/** Checks a network's arrays before the engine reads them: node index pairs, indices of existing nodes */
export const validateNetwork = (nodes: Float64Array, edges: Uint32Array): void => {
  if (nodes.length % 2 !== 0) throw new Error(`isard: network nodes must be lon, lat pairs (length ${nodes.length})`);
  if (edges.length % 2 !== 0) throw new Error(`isard: network edges must be node index pairs (length ${edges.length})`);

  const count = nodes.length / 2;

  for (let i = 0; i < edges.length; i += 1) {
    const node = edges[i] as number;

    if (node >= count) throw new Error(`isard: network edge ${i >> 1} refers to node ${node}, of ${count} nodes`);
  }
};

/** The name of a basemap, checked: an unknown one throws */
export const checkBasemap = (basemap: Basemap): BasemapName => {
  const renamed = RENAMED_BASEMAPS[basemap];

  if (renamed) throw new Error(`isard: the basemap '${basemap}' is now named '${renamed}'`);
  if (basemap !== 'none' && !(basemap in VECTOR_BASEMAPS) && !(basemap in BASEMAPS)) {
    throw new Error(`isard: unknown basemap '${basemap}'`);
  }

  return basemap;
};
