# Examples

Each folder is a page: `index.html` and its `main.ts`, importing `@ign-junn/isard` as an application does. The data
comes from the Géoplateforme (WFS of BD TOPO and ADMIN EXPRESS, vector tiles, WMS of the relief), no key needed.

| Example | Shows |
|---|---|
| [`globe`](globe/main.ts) | the globe turning in its atmosphere, a flight to a click |
| [`basic`](basic/main.ts) | a map, the vector basemap, controls, a draggable marker and its popup |
| [`network`](network/main.ts) | a network recolored at every move: walking times from the cursor, computed in the page |
| [`geojson`](geojson/main.ts) | fills colored by values through a ramp, outlines, labels, picking |
| [`buildings`](buildings/main.ts) | buildings raised to their height, colored by it, picked |
| [`relief`](relief/main.ts) | the relief, a sky, and a fragment shader of the page drawn over the ground |
| [`tiles3d`](tiles3d/main.ts) | a photomesh streamed as 3D Tiles |
| [`vector`](vector/main.ts) | the PLAN.IGN vector styles, a restyled subset on its own layer |

`bun run dev` at the root builds them and serves them on http://127.0.0.1:5180/examples/. They are also live in the
documentation, next to their code.
