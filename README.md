# geo-render

**A WebGL2 map engine for large, interactive data layers on Géoplateforme basemaps.** Rust core compiled to
WebAssembly, small TypeScript API. Built for maps where millions of features change at every mouse move
(accessibility, flooding), where a general-purpose map library spends its time on things these maps do not need.

```ts
import { GeoMap } from 'geo-render';

const map = await GeoMap.create(document.getElementById('map')!, { center: [2.4246, 48.8457], zoom: 14 });
const network = map.addNetwork({ nodes, edges, max: 1800, contour: 300 }); // lon/lat pairs, node index pairs
map.on('hover', ({ lonLat }) => lonLat && network.setValues(travelTimes(lonLat))); // one value per node
```

That is the whole integration: the map fills its container, follows its size, handles mouse and touch, and loads
the Géoplateforme basemap (PLAN.IGN grey by default, hillshade on top), on a globe that turns into Web Mercator as
you zoom in.

## Why it is fast

- **Geometry uploaded once.** A network layer keeps node positions in a float texture and edges as instances. New
  values are one texture upload (4 bytes per node), then **one draw call** for the whole layer: 2 M segments
  recolor in the same frame as the mouse move.
- **One draw call for the whole raster stack.** All the tiles of a layer share a texture array (fixed pool, least
  recently used slots recycled); images are decoded by the browser off the main thread and uploaded straight into
  their slot. Consecutive raster layers (up to four: basemap, hillshade…) are composited in the fragment shader, as
  iTowns does: each pixel is shaded once, and a tile fading in is mixed with its stand-in in the same pass.
- **Frames on demand only.** Nothing runs while the map is idle. Pointer events are coalesced to one per display
  frame, and values computed in a `hover` listener are drawn by that very frame.
- **f64 on the CPU, f32 relative to the camera on the GPU.** Exact to the centimetre at any zoom, without the
  precision tricks of 32-bit world coordinates.
- **Small.** 266 KB of WebAssembly (88 KB gzipped) and 27 KB of JavaScript.
- **No holes, no backlog.** A loading tile is replaced by its nearest loaded ancestor (or its loaded descendants, up
  to three levels, when zooming out); where nothing covers an area yet, a tile two levels coarser is loaded first
  (shallow first, as in 3DTilesRendererJS) and new tiles fade in over their stand-in. Requests for tiles that left
  the view are aborted, transient errors (network, rate limit, server) are retried with backoff and past the HTTP
  cache (the Géoplateforme sends its error answers with a 21-day cache lifetime), and a tile the
  server does not have (404) is covered by its ancestor. Arrived tiles are uploaded a few per frame so a burst never
  stalls a frame. Tilted views load fine tiles near the camera and coarse ones towards the horizon.
- **Downloads go where the eye is.** Tiles near the screen center load first; while the camera moves, covered
  tiles off the center wait until it rests (foveated loading, as in Cesium). During a camera flight (`easeTo`),
  the levels flown through are skipped (Cesium's skip LOD) and the destination loads in advance: a flight from the
  whole globe to a street makes 5 times fewer requests and lands with its tiles ready.
- **Picking at any scale.** `map.pick` draws the network layers once more into an offscreen target, each edge colored
  by its number, clipped to the pixel under the cursor, and reads that pixel back (as deck.gl does).
- **3D Tiles textured meshes.** `map.addTiles3d({ url })` streams photogrammetry and buildings (3D Tiles 1.0 b3dm, 1.1
  glb, external tilesets), compressed or not: meshopt and mesh quantization decoded in Rust, Draco geometry and KTX2
  (Basis Universal) textures decoded in the workers by the official modules, loaded on first use; KTX2 textures stay
  compressed on the GPU (ASTC, BC7, ETC2 or S3TC, whichever it supports). Traversal by screen-space error, children replacing their parent only once all are ready
  (no holes), coarse tiles first, downloads aborted when they leave the view, a GPU memory budget, mipmapped textures
  decoded off the main thread. Vertices stay in f32 around each tile's center and a matrix computed in f64 maps them
  to the view, on the plane and on the globe. The demo streams the Lille photomesh (MEL 2020, 5 cm).
- **Space around the globe.** Stars fixed to the Earth's frame (they turn with the globe) and an atmosphere halo,
  procedural in one full-screen triangle, fading out as the globe turns flat (`sky: false` to turn it off).
- **Globe for free.** The globe is a sphere up to zoom 5, blending into Web Mercator up to zoom 7; above, it costs
  exactly what the flat map costs (tiles are two triangles again). Tiles behind the horizon are never requested.

## API

| | |
|---|---|
| `GeoMap.create(container, options)` | `center`, `zoom`, `pitch`, `bearing`, `projection` (`'globe'`, `'mercator'`), `loading` (`'immediate'`, `'smooth'`), `background`, `workers`, `workerUrl`, `basemap` (`'plan-grey'`, `'plan'`, `'ortho'`, `'none'`), `hillshade` (opacity), `minZoom`, `maxZoom`, `interactive`, `wasmUrl` |
| `map.on(type, listener)` | `'move'` (camera), `'hover'` (point, lonLat; once per frame), `'click'`, `'render'` (engine ms); returns an unsubscribe function |
| `map.getCamera()`, `setCamera(c)`, `easeTo(c, ms)` | camera conventions of MapLibre: settings carry over unchanged |
| `map.project(lonLat)`, `map.unproject(point)` | CSS pixels ↔ degrees |
| `map.addTiles3d({ url, maxScreenSpaceError, opacity })` | textured meshes from a `tileset.json`; `layer.setStyle(…)`, `layer.stats()`, `layer.remove()` |
| `map.pick(point)` | `{ layer, index }` of the network edge under a screen point: one pixel of an offscreen picking pass, cheap at any scale |
| `map.setBasemap(basemap, hillshade)` | swaps the basemap under the other layers |
| `map.setProjection(projection)` | `'globe'` or `'mercator'` |
| `map.setLoading(mode)` | `'immediate'` (default): everything visible loads, tiles show as soon as they arrive; `'smooth'`: levels skipped during flights, off-center tiles waiting while the camera moves, fade-in, fewer downloads |
| `map.setBackground(color)` | `#rrggbb` behind the map |
| `map.stats()` | tile counters per raster layer: drawn, kept on the GPU, loading, waiting for upload |
| `map.addRaster({ url, minZoom, maxZoom, opacity, saturation })` | any XYZ/WMTS Web Mercator tiles (`{z}`, `{x}`, `{y}`) |
| `map.addNetwork({ nodes, edges, width, max, contour, opacity })` | `layer.setValues(Float32Array)` (negative = unreached), `layer.setStyle(…)`, `layer.remove()` |

Controls: drag to pan (with inertia), wheel or double click to zoom (shift + double click out), right drag or
ctrl/shift + drag to rotate and tilt, two fingers to pinch, turn and pan.

## Develop

Requires Rust (with the `wasm32-unknown-unknown` target), `wasm-bindgen-cli` at the version pinned in
`crates/engine/Cargo.toml`, and Bun.

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129
bun install
bun run dev        # builds, then serves the demo on http://127.0.0.1:5180 and the benchmark on /bench/ (Ctrl+C stops it; PORT to change)
bun run test       # Rust tests: camera, projections, tile selection, 3D Tiles traversal and decoding
bun run lint       # Biome (TypeScript), clippy pedantic (native and wasm)
bun run typecheck  # strict TypeScript: package, worker, demo, scripts
bun run format     # Biome and rustfmt
```

Demo URL parameters: `?projection=mercator`, `?workers=0` (3D Tiles decoded on the main thread, to compare).

```
crates/engine/          Rust core (crate geo-render), compiled to WebAssembly
  src/geo/              camera and projections, WGS 84 geodesy, tile selection        pure, tested natively
  src/tiles3d/          3D Tiles tree, traversal and decoding (pure, tested); WebGL layer; worker entry point
  src/render/           WebGL2 layers: raster stack, network, globe base, helpers
  src/engine.rs         the Engine exported to JavaScript, with its TypeScript types
  tests/data/           a tile of the Lille photomesh (MEL open data)
packages/geo-render/    the npm package: GeoMap, layers, interactions, decoding workers
apps/demo/              the demo, importing geo-render like any application
apps/bench/             benchmark: geo-render, iTowns and 3DTilesRendererJS on the same scenario
scripts/                build and demo server
```

Everything is typed end to end: the engine declares the TypeScript types of what it returns (`Camera`, `PickHit`,
`RasterLayerStats`, `Tiles3dStats`, `TileLoaderLike`…), the package reuses them, the worker is checked against the
WebWorker library, all under `strict`, `noUncheckedIndexedAccess` and `exactOptionalPropertyTypes`.

## Benchmark

`bun run dev`, then open http://127.0.0.1:5180/bench/ (or the "Benchmark" link of the demo) and press **Run the
benchmark**. The same scenario plays with each engine in turn, in a 1280 × 800 frame:

| Engine | Setup |
|---|---|
| geo-render | this repository |
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

## Roadmap

1. Vector basemap (PLAN.IGN vector tiles: fills, lines, then labels), to restyle the ground per use case.
2. Terrain (RGE ALTI); decoding of 3D Tiles in workers; KTX2 / Draco / meshopt; BD TOPO buildings colored by value.
3. Field layer: a value grid drawn as a smooth surface (water depth over the terrain for flooding).
4. Picking (feature under the cursor) and points (stations).
5. Move geo-reach and geo-flood onto the engine.
