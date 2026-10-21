/**
 * Travel times on hover: the streets of the IGN's BD TOPO (Géoplateforme WFS) as a network layer, over PLAN.IGN, the walking time from the cursor to every node
 * worked out in the page (Dijkstra), handed to the map in one call. The map calls `hover` in the frame it draws: the
 * new times are drawn by that very frame.
 */
import { GeoMap, geoplateformeWfs } from 'isard';
import { element } from '../shared/page.ts';

const BOX: [number, number, number, number] = [2.405, 48.835, 2.445, 48.857];
/** Walking speed, metres per second */
const WALK = 1.25;

const map = await GeoMap.create(element('map'), {
  center: [2.425, 48.846],
  zoom: 15,
  basemap: 'ign-plan-grey-vector',
  projection: 'mercator',
});

// The graph: a node per distinct end of a street, an edge per street, along its line
const roads = await geoplateformeWfs<{ nature: string }>('BDTOPO_V3:troncon_de_route', BOX);
const index = new Map<string, number>();
const nodes: number[] = [];
const node = ([lon, lat]: number[]) => {
  const key = `${lon?.toFixed(7)},${lat?.toFixed(7)}`;
  let i = index.get(key);
  if (i === undefined) {
    i = nodes.length / 2;
    index.set(key, i);
    nodes.push(lon ?? 0, lat ?? 0);
  }
  return i;
};
const edges: number[] = [];
const start = [0];
const points: number[] = [];
const lengths: number[] = [];
const metres = (a: number[], b: number[]) => {
  const k = Math.cos(((a[1] ?? 0) * Math.PI) / 180);
  return Math.hypot(((b[0] ?? 0) - (a[0] ?? 0)) * k, (b[1] ?? 0) - (a[1] ?? 0)) * 111_320;
};
roads.features.forEach((f) => {
  if (f.geometry?.type !== 'LineString') return;
  const line = f.geometry.coordinates;
  const [first, last] = [line[0], line.at(-1)];
  if (!first || !last) return;
  edges.push(node(first), node(last));
  line.forEach((p) => {
    points.push(p[0], p[1]);
  });
  start.push(points.length / 2);
  lengths.push(line.slice(1).reduce((s, p, i) => s + metres(line[i] ?? p, p), 0));
});
const count = nodes.length / 2;
element('nodes').textContent = count.toLocaleString('en');

// Seconds to walk each edge, either way
const costs = new Float32Array(lengths.flatMap((l) => [l / WALK, l / WALK]));
const network = map.addNetwork({
  nodes: Float64Array.from(nodes),
  edges: Uint32Array.from(edges),
  lines: { start: Uint32Array.from(start), points: Float64Array.from(points) },
  costs,
  max: 1200,
  contour: 120,
  width: [
    [13, 1.5],
    [17, 5],
  ],
  halo: 30,
});

// Adjacency, for the runs
const around: number[][] = Array.from({ length: count }, () => []);
for (let e = 0; e < edges.length / 2; e += 1) {
  around[edges[e * 2] ?? 0]?.push(e);
  around[edges[e * 2 + 1] ?? 0]?.push(e);
}

/** Seconds from the node nearest to a point to every node (a plain Dijkstra over a binary heap) */
const times = (lon: number, lat: number): Float32Array => {
  let source = 0;
  let best = Number.POSITIVE_INFINITY;
  for (let i = 0; i < count; i += 1) {
    const d = metres([lon, lat], [nodes[i * 2] ?? 0, nodes[i * 2 + 1] ?? 0]);
    if (d < best) [best, source] = [d, i];
  }
  const t = new Float32Array(count).fill(Number.POSITIVE_INFINITY);
  t[source] = best / WALK;
  const heap: [number, number][] = [[t[source] ?? 0, source]];
  while (heap.length) {
    const top = heap[0];
    const last = heap.pop();
    if (!top || !last) break;
    if (heap.length) {
      heap[0] = last;
      for (let i = 0; ; ) {
        const [l, r] = [2 * i + 1, 2 * i + 2];
        let m = i;
        if ((heap[l]?.[0] ?? Infinity) < (heap[m]?.[0] ?? Infinity)) m = l;
        if ((heap[r]?.[0] ?? Infinity) < (heap[m]?.[0] ?? Infinity)) m = r;
        if (m === i) break;
        [heap[i], heap[m]] = [heap[m] as [number, number], heap[i] as [number, number]];
        i = m;
      }
    }
    const [time, n] = top;
    if (time > (t[n] ?? 0)) continue;
    for (const e of around[n] ?? []) {
      const other = edges[e * 2] === n ? (edges[e * 2 + 1] ?? 0) : (edges[e * 2] ?? 0);
      const next = time + (costs[e * 2] ?? 0);
      if (next < (t[other] ?? 0)) {
        t[other] = next;
        heap.push([next, other]);
        for (let i = heap.length - 1; i > 0; ) {
          const p = (i - 1) >> 1;
          if ((heap[p]?.[0] ?? 0) <= next) break;
          [heap[i], heap[p]] = [heap[p] as [number, number], heap[i] as [number, number]];
          i = p;
        }
      }
    }
  }
  return t;
};

map.on('hover', ({ lonLat }) => {
  if (!lonLat) return;
  const t0 = performance.now();
  network.setValues(times(lonLat[0], lonLat[1]));
  element('ms').textContent = `${(performance.now() - t0).toFixed(1)} ms`;
});
network.setValues(times(2.425, 48.846));
