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
  the view are aborted, transient errors (network, rate limit, server) are retried with backoff, and a tile the
  server does not have (404) is covered by its ancestor. Arrived tiles are uploaded a few per frame so a burst never
  stalls a frame. Tilted views load fine tiles near the camera and coarse ones towards the horizon.
- **Downloads go where the eye is.** Tiles near the screen center load first; while the camera moves, covered
  tiles off the center wait until it rests (foveated loading, as in Cesium). During a camera flight (`easeTo`),
  the levels flown through are skipped (Cesium's skip LOD) and the destination loads in advance: a flight from the
  whole globe to a street makes 5 times fewer requests and lands with its tiles ready.
- **Picking at any scale.** `map.pick` draws the network layers once more into an offscreen target, each edge colored
  by its number, clipped to the pixel under the cursor, and reads that pixel back (as deck.gl does).
- **Globe for free.** The globe is a sphere up to zoom 5, blending into Web Mercator up to zoom 7; above, it costs
  exactly what the flat map costs (tiles are two triangles again). Tiles behind the horizon are never requested.

## API

| | |
|---|---|
| `GeoMap.create(container, options)` | `center`, `zoom`, `pitch`, `bearing`, `projection` (`'globe'`, `'mercator'`), `basemap` (`'plan-grey'`, `'plan'`, `'ortho'`, `'none'`), `hillshade` (opacity), `minZoom`, `maxZoom`, `interactive`, `wasmUrl` |
| `map.on(type, listener)` | `'move'` (camera), `'hover'` (point, lonLat; once per frame), `'click'`, `'render'` (engine ms); returns an unsubscribe function |
| `map.getCamera()`, `setCamera(c)`, `easeTo(c, ms)` | camera conventions of MapLibre: settings carry over unchanged |
| `map.project(lonLat)`, `map.unproject(point)` | CSS pixels ↔ degrees |
| `map.pick(point)` | `{ layer, index }` of the network edge under a screen point: one pixel of an offscreen picking pass, cheap at any scale |
| `map.setBasemap(basemap, hillshade)` | swaps the basemap under the other layers |
| `map.setProjection(projection)` | `'globe'` or `'mercator'` |
| `map.stats()` | tile counters per raster layer: drawn, kept on the GPU, loading, waiting for upload |
| `map.addRaster({ url, minZoom, maxZoom, opacity, saturation })` | any XYZ/WMTS Web Mercator tiles (`{z}`, `{x}`, `{y}`) |
| `map.addNetwork({ nodes, edges, width, max, contour, opacity })` | `layer.setValues(Float32Array)` (negative = unreached), `layer.setStyle(…)`, `layer.remove()` |

Controls: drag to pan (with inertia), wheel or double click to zoom (shift + double click out), right drag or
ctrl/shift + drag to rotate and tilt, two fingers to pinch, turn and pan.

## Develop

Requires Rust (with the `wasm32-unknown-unknown` target), `wasm-bindgen-cli` at the version pinned in
`crates/render/Cargo.toml`, and Bun.

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129
bun install
bun run dev     # builds, then serves the demo on http://127.0.0.1:5180 (?projection=mercator); Ctrl+C stops it
cargo test      # camera, projections and tile selection
```

| Path | |
|---|---|
| `crates/render` | engine: camera and projections, tile selection, WebGL2 layers (`raster`, `network`, `globe` base), the `Engine` exported to JavaScript |
| `js` | the `geo-render` package: `GeoMap`, interactions, render loop (`bun run build` → `js/dist`) |
| `demo` | basemaps on the globe, with the tile counters of each layer |

## Roadmap

1. Vector basemap (PLAN.IGN vector tiles: fills, lines, then labels), to restyle the ground per use case.
2. Terrain (RGE ALTI) and BD TOPO buildings in 3D, colored by a value per building.
3. Field layer: a value grid drawn as a smooth surface (water depth over the terrain for flooding).
4. Picking (feature under the cursor) and points (stations).
5. Move geo-reach and geo-flood onto the engine.
