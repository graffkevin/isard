# Networks

The layer Isard was made for: a graph of millions of edges colored by a value per node, recolored at every mouse
move.

```ts
const network = map.addNetwork({
  nodes,                 // Float64Array: lon, lat of each node
  edges,                 // Uint32Array: node index pairs
  lines: { start, points }, // the line of each edge from A to B (else straight)
  costs,                 // Float32Array per edge: cost from A to B, then from B to A (Infinity: closed that way)
  max: 1800,             // values above are drawn as unreached
  width: [[12, 1.2], [15.5, 5]],
  halo: [[12, 46], [16, 10]], // a wide faint pass under the lines: a heat map that fills the blocks
  ramp: [[0, '#1a9850'], [0.5, '#fdae61'], [1, '#6a1b9a']], // fractions of max
  fade: 0.1,             // colors fading out before max
  contour: 300,          // a darker line every 300 value units
  edgeWidths,            // a width factor per edge: rail lines wider than streets
});

map.on('hover', ({ lonLat }) => lonLat && network.setValues(travelTimes(lonLat)));
```

- **Values per node**: `setValues(Float32Array)`, one upload of 4 bytes per node. Negative or from 1e30: unreached.
- **Costs per edge**: with costs, a point along an edge takes the quickest way there, min(value(A) + cost from A,
  value(B) + cost from B), worked out per pixel: the fronts meet where they should. Without, values are interpolated.
  `setCosts(…)` changes the profile (walk, bike, car) with the geometry kept on the graphics card.
- **Only the view drawn**: the pieces are sorted by square; of 2 million pieces around Paris, about 18,000 are drawn
  at zoom 14.
- **Picking**: `map.pickFeature(point)` gives the edge under the cursor.
- **Buildings** can read the same values on the graphics card: see [Buildings](./buildings).

<LiveExample slug="network" height="440px" />
