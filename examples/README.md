# Examples

Each folder is a page: `index.html` and its `main.ts`, importing `isard` as an application does. The default
basemap is OpenFreeMap's (OpenStreetMap data); the French examples take the IGN's data from the Géoplateforme (WFS of
BD TOPO and ADMIN EXPRESS, PLAN.IGN, orthophotos, WMS of the relief), the Moon's and Mars's come from NASA's Trek, no key
needed.

| Example | Shows |
|---|---|
| [`globe`](globe/main.ts) | the globe turning in its atmosphere, a flight to a click |
| [`basic`](basic/main.ts) | a map, the vector basemap, controls, a draggable marker and its popup |
| [`network`](network/main.ts) | a network recolored at every move: walking times from the cursor, computed in the page |
| [`geojson`](geojson/main.ts) | fills colored by values through a ramp, outlines, labels, picking |
| [`buildings`](buildings/main.ts) | buildings raised to their height, colored by it, picked |
| [`relief`](relief/main.ts) | the relief, a sky, and a fragment shader of the page drawn over the ground |
| [`tiles3d`](tiles3d/main.ts) | a photomesh streamed as 3D Tiles |
| [`models`](models/main.ts) | glTF models at positions: a row of wind turbines on the relief (one instanced draw), textured trucks, picked |
| [`threejs`](threejs/main.ts) | a three.js scene in a custom layer, sharing the depth of the buildings |
| [`vector`](vector/main.ts) | OpenFreeMap's vector styles, a restyled subset on its own layer |
| [`lambert93`](lambert93/main.ts) | data in Lambert-93: PLAN IGN reprojected from its Lambert-93 tiles, the arrondissements from a WFS in EPSG:2154, a GPX file parsed |
| [`moon`](moon/main.ts) | a map of the Moon: NASA's LRO mosaic reprojected from longitudes and latitudes over LOLA's relief, the Apollo sites, Tycho and Copernicus in 3D |
| [`mars`](mars/main.ts) | a map of Mars in its dusty halo: the Viking colour mosaic over MOLA's relief, Olympus Mons and Valles Marineris in 3D, the rovers |

`bun run dev` at the root builds them and serves them on http://127.0.0.1:5180/examples/. They are also live in the
documentation, next to their code.
