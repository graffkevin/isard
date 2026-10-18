# Performance

Isard is built for maps where everything changes at every mouse move. What makes it fast:

- **Geometry uploaded once, only the view drawn.** A network is cut into pieces sorted by square of a 256 × 256 grid;
  each frame draws only the squares in view (a quadtree descent against the frustum, a few draw calls for runs of
  squares). Of 2 million pieces around Paris, about 18,000 are drawn at zoom 14.
- **Values in textures.** New values are one upload of 4 bytes per node (or per feature); colors, ramps, widths,
  filters are read by the shaders. Restyling never touches the geometry.
- **Worked out per pixel.** Along an edge with costs, a point takes the quickest way there, min(t(A) + cost from A,
  t(B) + cost from B): an edge reached from both ends shows where the two fronts meet.
- **One draw call for the ground.** The raster layers of a stack (basemap, hillshade…) share texture arrays and are
  composited in one fragment shader, each pixel shaded once.
- **Frames on demand.** Nothing runs while idle, a mouse moving over the map included: a frame is drawn for the
  pointer only when something listens to `hover` or `hoverfeature`, and the hover listener runs in the frame it
  draws. `pick` and `pickFeature` wait for the GPU: on hover, `hoverfeature` picks without waiting.
- **Work off the main thread.** Images decoded by the browser, 3D Tiles decoded in workers (Draco, meshopt, KTX2),
  vector tiles drawn in workers on CPU canvases (an opaque tile moved out of its canvas without a copy, no read-back
  on the main thread at the upload), each worker keeping its share of the decoded tiles. Still on the main thread:
  the Terrarium and Terrain-RGB relief (each PNG decoded by the browser, its colors turned into altitudes in the
  page: 256 × 256 pixels a tile) and the reprojection of equirectangular rasters (a Canvas 2D drawing per tile).
- **Less WebAssembly for most maps.** The engine comes in two builds: without 3D Tiles (`isard_core_bg.wasm`, 220 KB
  gzipped), loaded by every map, and with them (`isard_bg.wasm`, 330 KB), loaded for the first 3D Tiles layer. That
  move costs a download and a stall of a frame or two, the map being rebuilt on the new engine; `tiles3d: true`
  avoids it for a map that shows 3D Tiles at once, `tiles3d: 'preload'` downloads and compiles the full engine once
  the map is idle (see [3D Tiles](../layers/tiles3d#the-engine-with-3d-tiles)).
- **No holes, no backlog.** Missing tiles covered by their nearest ancestor or descendants, coarse tiles first, stale
  requests aborted, transient errors retried, arrivals uploaded within a budget per frame, foveated loading and skipped
  levels during flights.

## Measured

geo-reach, an accessibility map of the French regions with their transit networks, on MapLibre and on Isard (a
trial branch), on an Intel UHD (TGL GT1), Chromium, 1280 × 800:

| | MapLibre | Isard |
|---|---|---|
| Ready | 8.5 – 9.9 s | 6.7 – 8.4 s |
| Hover sweep, frames over 33 ms | 5 – 7 % | 2.7 – 8 % |
| Hover sweep, 95th percentile | 30 – 37 ms | 18 – 35 ms |
| JavaScript downloaded | 1.25 MB + 508 KB worker | 285 KB + 654 KB of WebAssembly |

## The benchmark of the repository

`bun run dev`, then open http://127.0.0.1:5180/bench/ (or the "Benchmark" link of the demo) and press **Run the
benchmark**. The same scenario plays with each engine in turn, in a 1280 × 800 frame:

| Engine | Setup |
|---|---|
| Isard | this repository |
| iTowns 2.46.1-next.72 | the version of junn-apps, with 3d-tiles-renderer 0.5.3 forced as junn-apps does (`overrides`): with its own 0.4.28 the 3D Tiles queue never starts |
| 3DTilesRendererJS 0.5.3 | three.js 0.182, ortho through `XYZTilesOverlay` on a `GeneratedSurfacePlugin` ellipsoid, continuous rendering as in its examples |

Same data (Géoplateforme ortho, Lille photomesh), same screen-space error (8 px), same field of view (36.87°), poses
given as target, distance, tilt and heading and converted for each engine. Phases: view of France until complete,
6 s flight to Lille, wait until complete, 10 s orbit, wait until complete, 6 s flight back.

Measured per phase on the main thread: frames per second, frame interval (median, 95th and 99th percentiles), share of
frames over 33 ms, long tasks (over 50 ms), time until the basemap, the 3D mesh and both are complete; and the
JavaScript heap.

Two rounds, about 10 minutes: the first warms the servers and the browser cache (the Lille mesh server answers a cold
file in about 15 s, which would penalize whichever engine runs first), the second, in reverse order, is the reference.

Run it on a real GPU: in a headless browser rendering on the CPU (SwiftShader), the GPU-heavy engines are penalized
far beyond reality, and 3DTilesRendererJS sometimes draws a black frame there. Pages can be opened alone, holding a pose
to check what an engine shows: `/bench/itowns.html?still=lille` (`still=france`, `?logdepth=1` for 3DTilesRendererJS).
