/**
 * Travel times on hover in React: the streets of the IGN's BD TOPO (Géoplateforme WFS) as a network layer, over
 * PLAN.IGN, the walking time from the cursor to every node worked out in the page (Dijkstra), handed to the map in one
 * call. The map calls `hover` in the frame it draws: the new times, set on the layer's handle rather than through a
 * state, are drawn by that very frame.
 */
import { type FeatureCollection, geoplateformeWfs, type NetworkLayer } from 'isard';
import { IsardMap, Layer, useMapEvent } from 'isard/react';
import { useEffect, useMemo, useRef, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { element } from '../../shared/page.ts';

const BOX: [number, number, number, number] = [2.405, 48.835, 2.445, 48.857];
/** Walking speed, metres per second */
const WALK = 1.25;
const WIDTH: [number, number][] = [
  [13, 1.5],
  [17, 5],
];

type Roads = FeatureCollection<{ nature: string }>;
type Graph = ReturnType<typeof graphOf>;

const metres = (a: number[], b: number[]) => {
  const k = Math.cos(((a[1] ?? 0) * Math.PI) / 180);

  return Math.hypot(((b[0] ?? 0) - (a[0] ?? 0)) * k, (b[1] ?? 0) - (a[1] ?? 0)) * 111_320;
};

/** The graph: a node per distinct end of a street, an edge per street, along its line */
const graphOf = (roads: Roads) => {
  const index = new Map<string, number>();
  const nodes: number[] = [];
  const node = ([lon, lat]: number[]) => {
    const key = `${lon?.toFixed(7)},${lat?.toFixed(7)}`;
    const known = index.get(key);

    if (known !== undefined) return known;

    const i = nodes.length / 2;

    index.set(key, i);
    nodes.push(lon ?? 0, lat ?? 0);

    return i;
  };
  const edges: number[] = [];
  const start = [0];
  const points: number[] = [];
  const lengths: number[] = [];

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
  // Adjacency, for the runs
  const around: number[][] = Array.from({ length: count }, () => []);

  for (let e = 0; e < edges.length / 2; e += 1) {
    around[edges[e * 2] ?? 0]?.push(e);
    around[edges[e * 2 + 1] ?? 0]?.push(e);
  }

  return {
    count,
    nodes: Float64Array.from(nodes),
    edges: Uint32Array.from(edges),
    lines: { start: Uint32Array.from(start), points: Float64Array.from(points) },
    // Seconds to walk each edge, either way
    costs: new Float32Array(lengths.flatMap((l) => [l / WALK, l / WALK])),
    around,
  };
};

/** Seconds from the node nearest to a point to every node (a plain Dijkstra over a binary heap) */
const times = ({ count, nodes, edges, costs, around }: Graph, lon: number, lat: number): Float32Array => {
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

/** The streets, recolored from the cursor at each move; the duration of the run told to `onRun` */
const Streets = ({ graph, onRun }: { graph: Graph; onRun: (ms: number) => void }) => {
  const network = useRef<NetworkLayer>(undefined);

  useMapEvent('hover', ({ lonLat }) => {
    if (!lonLat) return;

    const t0 = performance.now();

    network.current?.setValues(times(graph, lonLat[0], lonLat[1]));
    onRun(performance.now() - t0);
  });

  return (
    <Layer
      ref={network}
      type="network"
      nodes={graph.nodes}
      edges={graph.edges}
      lines={graph.lines}
      costs={graph.costs}
      max={1200}
      contour={120}
      width={WIDTH}
      halo={30}
      onLayer={(layer) => layer.setValues(times(graph, 2.425, 48.846))}
    />
  );
};

const App = () => {
  const [roads, setRoads] = useState<Roads>();
  const [ms, setMs] = useState<number>();

  // The road sections of the box, fetched once
  useEffect(() => {
    geoplateformeWfs<{ nature: string }>('BDTOPO_V3:troncon_de_route', BOX).then(setRoads);
  }, []);

  const graph = useMemo(() => roads && graphOf(roads), [roads]);

  return (
    <>
      <IsardMap id="map" center={[2.425, 48.846]} zoom={15} basemap="ign-plan-grey-vector" projection="mercator">
        {graph && <Streets graph={graph} onRun={setMs} />}
      </IsardMap>
      <div className="panel">
        <h1>Travel times on hover</h1>
        <p>
          The streets of BD TOPO around Saint-Mandé, recolored at every move: walking time from the cursor, computed in
          the page, drawn in the same frame.
        </p>
        <p>
          Nodes <output id="nodes">{graph ? graph.count.toLocaleString('en') : '…'}</output> · run{' '}
          <output id="ms">{ms === undefined ? '…' : `${ms.toFixed(1)} ms`}</output>
        </p>
      </div>
    </>
  );
};

createRoot(element('root')).render(<App />);
