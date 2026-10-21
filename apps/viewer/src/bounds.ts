/** The bounds of GeoJSON, shared by the viewer and its server (scripts/viewer.ts): no dependency */
import type { Bounds, GeoJsonCollection } from './protocol.ts';

/** The bounds of a feature collection's coordinates (west, south, east, north), undefined if it has no position */
export const collectionBounds = (data: GeoJsonCollection): Bounds | undefined => {
  const b: Bounds = [Infinity, Infinity, -Infinity, -Infinity];
  const visit = (c: unknown): void => {
    if (!Array.isArray(c)) return;
    if (typeof c[0] === 'number' && typeof c[1] === 'number') {
      b[0] = Math.min(b[0], c[0]);
      b[1] = Math.min(b[1], c[1]);
      b[2] = Math.max(b[2], c[0]);
      b[3] = Math.max(b[3], c[1]);

      return;
    }
    for (const x of c) visit(x);
  };
  const geometry = (g: unknown): void => {
    if (typeof g !== 'object' || g === null) return;
    if ('coordinates' in g) visit(g.coordinates);
    if ('geometries' in g && Array.isArray(g.geometries)) g.geometries.forEach(geometry);
  };

  (data.features ?? []).forEach((f) => {
    geometry(f.geometry);
  });

  return Number.isFinite(b[0]) ? b : undefined;
};
