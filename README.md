# Isard

[![npm](https://img.shields.io/npm/v/@ign-junn/isard)](https://www.npmjs.com/package/@ign-junn/isard)

**A WebGL2 map engine for large, interactive data layers on Géoplateforme basemaps.** Rust core compiled to
WebAssembly, small TypeScript API. Built for maps where millions of features change at every mouse move
(accessibility, flooding), where a general-purpose map library spends its time on things these maps do not need:
networks recolored at every move, buildings colored by value, water depth drawn over the relief, labels, 3D Tiles,
on raster or vector PLAN.IGN basemaps.

```sh
bun add @ign-junn/isard     # or npm install @ign-junn/isard
```

```ts
import { GeoMap } from '@ign-junn/isard';

const map = await GeoMap.create(document.getElementById('map')!, { center: [2.4246, 48.8457], zoom: 14 });
const network = map.addNetwork({ nodes, edges, max: 1800, contour: 300 }); // lon/lat pairs, node index pairs
map.on('hover', ({ lonLat }) => lonLat && network.setValues(travelTimes(lonLat))); // one value per node
```

That is the whole integration: the map fills its container, follows its size, handles mouse and touch, and loads
the Géoplateforme basemap (PLAN.IGN grey by default, hillshade on top), on a globe that turns into Web Mercator as
you zoom in.

The package is one ES module, the engine (`isard_bg.wasm`) and two workers (`decoder.worker.js` for 3D Tiles,
`basemap.worker.js` for vector basemaps), found next to the module by default. A bundler that moves files around is
given their URLs, with Vite for instance:

```ts
import { GeoMap } from '@ign-junn/isard';
import basemapWorkerUrl from '@ign-junn/isard/basemap.worker.js?url';
import decoderWorkerUrl from '@ign-junn/isard/decoder.worker.js?url';
import wasmUrl from '@ign-junn/isard/isard_bg.wasm?url';

const map = await GeoMap.create(container, {
  basemap: 'plan-grey-vector',
  wasmUrl,
  workerUrl: decoderWorkerUrl,
  basemapWorkerUrl,
});
```

and kept out of its dependency pre-bundling (`optimizeDeps: { exclude: ['@ign-junn/isard'] }` in Vite), so that the
files keep their place. A layer drawn with the map's own WebGL context: see `addGround` (GLSL over the ground).

## Why it is fast

- **Geometry uploaded once, only the view drawn.** A network layer cuts its edges (straight, or along their lines)
  into pieces, one instance each, sorted by square of a 256 × 256 grid over the network; the nodes and costs of each
  edge and the values of the nodes live in textures. New values are one texture upload (4 bytes per node), and each
  frame draws only the squares in view (a quadtree descent against the frustum, a few draw calls for the runs of
  consecutive squares): of 2 M pieces around Paris, about 18,000 are drawn at zoom 14.
- **Values worked out per pixel.** With edge costs, a point of an edge takes the quickest way there, min(t(A) + cost
  from A, t(B) + cost from B), as a travel time does: an edge reached from both ends shows where the two fronts meet,
  and an unreached node colors nothing. A faint wide halo pass under the lines adds up into a heat map.
- **One draw call for the whole raster stack.** All the tiles of a layer share a texture array (fixed pool, least
  recently used slots recycled); images are decoded by the browser off the main thread and uploaded straight into
  their slot. Consecutive raster layers (up to four: basemap, hillshade…) are composited in the fragment shader, as
  iTowns does: each pixel is shaded once, and a tile fading in is mixed with its stand-in in the same pass.
- **Frames on demand only.** Nothing runs while the map is idle. Pointer events are coalesced to one per display
  frame, and values computed in a `hover` listener are drawn by that very frame.
- **f64 on the CPU, f32 relative to the camera on the GPU.** Exact to the centimetre at any zoom, without the
  precision tricks of 32-bit world coordinates.
- **Small.** 640 KB of WebAssembly (280 KB gzipped) and 100 KB of JavaScript (32 KB gzipped), plus the workers.
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
- **Relief.** `terrain: true` lifts the ground with the IGN terrain model (Géoplateforme WMS, raw float altitudes),
  three levels coarser than the imagery, in the vertex shader, with skirts against cracks; the camera orbits the
  ground under the screen center. Altitudes get the RAF20 geoid height, so 3D Tiles meshes (ellipsoidal heights) sit
  on the ground. Network layers are draped: a page table over the network's extent gives each vertex the deepest
  loaded elevation tile, edges are split in pieces that follow the ground, and the hills hide the lines behind them.
  Picking and dragging work on the relief itself: the ground is drawn once more into the picking target, its depth
  packed in the color and read back under the cursor; the grabbed point stays under it while the eye moves at a
  constant height, and when finer relief arrives the orbited point slides to the ground without moving the eye.
- **Space around the globe.** Stars fixed to the Earth's frame (they turn with the globe) and an atmosphere halo,
  procedural in one full-screen triangle, fading out as the globe turns flat (`sky: false` to turn it off).
- **Globe for free.** The globe is a sphere up to zoom 5, blending into Web Mercator up to zoom 7; above, it costs
  exactly what the flat map costs (tiles are two triangles again). Tiles behind the horizon are never requested.

## API

| | |
|---|---|
| `GeoMap.create(container, options)` | `center`, `zoom`, `pitch`, `bearing`, `projection` (`'globe'`, `'mercator'`), `loading` (`'immediate'`, `'smooth'`), `background`, `workers`, `workerUrl`, `basemap` (`'plan-grey'`, `'plan'`, `'ortho'`, `'none'`), `hillshade` (opacity), `terrain` (`true` or `{ url, exaggeration, maxZoom, geoid }`), `atmosphere`, `minZoom`, `maxZoom`, `maxPitch` (up to 85), `interactive`, `attributionControl` (`false`, or `{ compact, customAttribution }`), `wasmUrl` |
| `map.on(type, listener)` | `'movestart'`, `'move'`, `'moveend'` (camera), `'resize'`, `'hover'` (point, lonLat; once per frame), `'click'`, `'render'` (engine ms), `'idle'` (everything in view loaded and drawn: the end of a load), `'leave'` (the pointer left the canvas); returns an unsubscribe function |
| `map.getCamera()`, `setCamera(c)`, `easeTo(c, ms or { duration, easing })`, `flyTo(c, { duration, curve, speed, easing })` | camera conventions of MapLibre: settings carry over unchanged; `flyTo` zooms out on its way (van Wijk and Nuij's path, as MapLibre) |
| `map.getBounds()`, `setMinZoom(z)`, `setMaxZoom(z)`, `setMaxPitch(p)` | `[west, south, east, north]` of the ground in view |
| `map.setAtmosphere(options)` | sky over the plane and fog of the ground for tilted views, with the settings of MapLibre's `setSky` (`skyColor`, `horizonColor`, `fogColor`, `skyHorizonBlend`, `horizonFogBlend`, `fogGroundBlend`) |
| `map.project(lonLat)`, `map.unproject(point)` | CSS pixels ↔ degrees |
| `map.getContainer()`, `map.getCanvas()` | the element given to `create` (made `position: relative` if static) and the canvas in it |
| `new Marker({ element, anchor, offset, draggable, className })` | DOM element at a position, placed after every frame (it follows the relief), hidden behind the camera; `.setLonLat(…)`, `.addTo(map)`, `.remove()`, `.setDraggable(b)` (dragged on the relief), `.setOffset(…)`, `.setPopup(popup)` (opened by a click), `.on('dragstart' \| 'drag' \| 'dragend', …)`; default: a round pin anchored at its tip |
| `new Popup({ closeButton, closeOnClick, maxWidth, className, anchor, offset })` | box with a tip pointing at a position, above it by default; `.setLonLat(…)`, `.setHTML(…)`, `.setText(…)`, `.setDOMContent(node)`, `.addTo(map)`, `.remove()`, `.isOpen()`, `.on('open' \| 'close', …)` |
| `map.addControl(control, position)`, `map.removeControl(control)` | anything with `onAdd(map)` returning an element and `onRemove(map)`, in a corner (`'top-right'` by default), stacked from the corner inward; built in: `NavigationControl({ showZoom, showCompass, visualizePitch })`, `AttributionControl({ compact, customAttribution })` (added by the map at the bottom right). Styled by `isard-` classes (`.isard-ctrl-group` for a box of 29 px buttons) and `--isard-…` custom properties |
| `map.addTiles3d({ url, maxScreenSpaceError, opacity })` | textured meshes from a `tileset.json`; `layer.setStyle(…)`, `layer.stats()`, `layer.remove()` |
| `map.pick(point, radius)`, `map.pickFeature(point, { radius, layers })` | `{ layer, index }` of the feature under a screen point (or the nearest within `radius` CSS pixels): a few pixels of an offscreen picking pass, cheap at any scale; fills are tested on the ground. `pickFeature` gives the layer handle and the GeoJSON feature |
| `map.addLines({ data, color, width, dash, … })` | GeoJSON lines (and polygon outlines): round caps and joins, dashes in line widths (`line-dasharray`), draped on the relief |
| `map.addCircles({ data, color, radius, strokeColor, strokeWidth, alignment, … })` | GeoJSON points as discs with a stroke, facing the screen or lying on the ground (`'map'`) |
| `map.addFills({ data, color, … })` | GeoJSON polygons, holes included; over the relief they lie on the ground, hills and valleys alike, at any size (a shadow volume: a world-sized veil with a hole is exact to the pixel) |
| `map.addExtrusions({ data, color, height, base, … })` | polygons raised into lit blocks with depth (buildings), darker towards their foot; in tiles (`layer.setTile(key, data)`, `removeTile(key)`) or one set; colored by their color, by values through the ramp, or by a network's node values (`linkNetwork(network, max)` with `setAnchors(…)`: two nodes and the cost from each, per building: they recolor with the network); in relief, the walls stand on the ground |
| `map.addSymbols({ data, text, font, size, color, haloColor, haloWidth, anchor, offset, maxWidth, lineHeight, overlap, … })` | text labels at points, at the middle of lines, at the center of polygons, in the Géoplateforme's fonts (signed distance fields, `glyphs` option for others): lines with `\n`, wrapping at `maxWidth` ems, runs of their own scale and font (`[{ text, scale, font }]`, as MapLibre's `format`), a halo; placed every frame without overlaps, all symbol layers sharing the collisions (the top layer first), unless `overlap` |
| `map.addGround({ code, anchor, bounds, textures, uniforms })` | the application's GLSL `vec4 ground(vec2 offset)` run over the ground itself: the ground tiles in its bounds drawn once more with it, so it lies on the relief like the basemap (water depth from a terrain grid and levels, heat maps); `offset` is the Mercator offset from `anchor`, exact at any zoom; textures in any format, integer ones included (`r16ui`…), 2D or 2D arrays; `layer.setTexture(…)`, `updateTexture(name, data, region)`, `setUniform(name, value)`, `setBounds(…)` |
| feature layer style | each value a constant, zoom stops `[[zoom, value], …]` (as `interpolate linear zoom`) or a function of the feature's properties; common to all: `opacity` (number or stops), `minZoom`, `maxZoom`, `visible`, `filter`, `ramp` (`[[value, color], …]`), `before` (a layer to go under); `layer.setData(…)`, `setStyle(…)`, `setValues(Float32Array)` (one per feature, colored by the ramp: one texture upload), `setFilter(fn)`, `setVisible(b)`, `moveBefore(layer)`, `remove()` |
| `map.setBasemap(basemap, hillshade)` | swaps the basemap under the other layers: rasters (`'plan'`, `'plan-grey'`, `'ortho'`) or vector tiles (`'plan-vector'`, `'plan-grey-vector'`: the PLAN.IGN styles drawn in workers into the raster pool, on the relief like any basemap, and their labels kept above the layers added later) |
| `map.vectorTiles({ style, workers })` | any MapLibre style (its legacy filters and functions, and the common expressions): `tiles.raster({ layers, extraLayers, size, before })` draws its fills, lines and circles (or a subset, restyled) as a raster layer; `tiles.labels({ layers })` its labels |
| `map.setProjection(projection)` | `'globe'` or `'mercator'` |
| `map.setLoading(mode)` | `'immediate'` (default): everything visible loads, tiles show as soon as they arrive; `'smooth'`: levels skipped during flights, off-center tiles waiting while the camera moves, fade-in, fewer downloads |
| `map.setBackground(color)` | `#rrggbb` behind the map |
| `map.stats()` | tile counters per raster layer: drawn, kept on the GPU, loading, waiting for upload |
| `map.addRaster({ url, minZoom, maxZoom, opacity, saturation, size, loader, before })` | any XYZ/WMTS Web Mercator tiles (`{z}`, `{x}`, `{y}`), or tiles drawn by the page (`loader(z, x, y, signal)` returning an `ImageBitmap` of `size` pixels) |
| `map.addNetwork({ nodes, edges, lines, costs, edgeWidths, width, max, contour, opacity, halo, haloOpacity, ramp, unreachedColor, fade })` | `lines`: `{ start, points }`, the line of each edge from A to B; `costs`: per edge, A → B then B → A (Infinity: closed); `width` and `halo` in CSS pixels or zoom stops; `edgeWidths`: a width factor per edge; `ramp`: `[[fraction of max, color], …]`; `fade`: share of `max` over which the colors fade out; `layer.setValues(Float32Array)` (negative or 1e30 = unreached), `layer.setCosts(…)` (new profile, geometry kept), `layer.setEdgeWidths(…)`, `layer.setStyle(…)`, `layer.stats()` (pieces, drawn), `layer.remove()` |

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
crates/geo/             camera and projections, WGS 84 geodesy, RAF20 geoid, tile selection    no WebGL, tested natively
crates/tiles3d/         3D Tiles tree, traversal and decoding                                 no WebGL, tested natively
  tests/data/           a tile of the Lille photomesh (MEL open data)
crates/engine/          WebAssembly core (crate isard), on the two above
  src/engine.rs         the Engine exported to JavaScript: camera, ground (raster layers over the relief), frame loop
  src/engine/pick.rs    picking: features and ground under a screen point
  src/layer.rs          the Layer trait: what is drawn over the ground (networks, 3D Tiles…)
  src/render/           WebGL2 layers and helpers; each Layer exports its own methods of Engine, next to it
  src/worker.rs         entry points of the 3D Tiles decoding workers
packages/isard/         the npm package @ign-junn/isard: GeoMap, layers, overlays, workers (3D Tiles, vector tiles)
apps/demo/              the demo, importing @ign-junn/isard like any application
apps/bench/             benchmark: isard, iTowns and 3DTilesRendererJS on the same scenario
scripts/                build, demo server, release
```

Everything is typed end to end: the engine declares the TypeScript types of what it returns (`Camera`, `PickHit`,
`RasterLayerStats`, `Tiles3dStats`, `TileLoaderLike`…), the package reuses them, the worker is checked against the
WebWorker library, all under `strict`, `noUncheckedIndexedAccess` and `exactOptionalPropertyTypes`.

## Release

`@ign-junn/isard` is published on npm, in the `ign-junn` organization (as `@ign-junn/design-system`). To publish a
version: set it in `packages/isard/package.json`, describe it in `CHANGELOG.md`, put an npm token in `.env` (see
`.env.example`), then:

```sh
bun run release:dry   # lint, types, tests, build, and the package packed without publishing: its file list
bun run release       # the same, then published
```

The package holds the build only (`dist`: module, types, engine, workers), with the README, the changelog and the
licence. The Rust crates (`isard-geo`, `isard-tiles3d`) are not published: they are compiled into the engine.

## Benchmark

`bun run dev`, then open http://127.0.0.1:5180/bench/ (or the "Benchmark" link of the demo) and press **Run the
benchmark**. The same scenario plays with each engine in turn, in a 1280 × 800 frame:

| Engine | Setup |
|---|---|
| isard | this repository |
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

Done: vector basemap (PLAN.IGN styles, labels above the data), buildings colored by value or by a network, ground
layers (water depth over the terrain), feature layers with picking, labels, markers, popups and controls.

1. Move geo-reach (running on it, on a trial branch) and geo-flood onto the engine.
2. Icons of the basemap's symbol layers (sprites), text along curved lines.
3. Labels fading in and out instead of popping.

## Licence

MIT, © Institut national de l'information géographique et forestière (IGN). See `LICENSE`.
