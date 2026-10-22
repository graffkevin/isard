---
aside: false
pageClass: wide-page
---

# Architecture

<ArchitectureDiagram />

Isard is a map engine in two halves: a **Rust core compiled to WebAssembly**, which owns the GPU (WebGL2), the camera
and every layer's state, and a **TypeScript SDK**, which owns the page (the canvas, events, overlays, workers,
network) and hands the core what it needs. This page follows the pieces from the outside in, then a frame and a tile
through them. The [layout of the repository](./guide/develop#layout) lists the folders.


## The engines

The core is built from four crates, three of them free of WebGL and I/O (tested natively):

| Crate | What it does |
|---|---|
| `crates/geo` | the camera and its projections (globe, Mercator, first person), the bodies (Earth, Moon, Mars), the RAF20 geoid, the selection of tiles in view, label placement, polygons |
| `crates/tiles3d` | the 3D Tiles tree and its traversal (refinement, implicit tiling), the decoding of b3dm, pnts and glb (meshopt, quantization, metadata) |
| `crates/pointcloud` | COPC point clouds: the LAS header, the octree hierarchy, its traversal; the LAZ decoder built apart |
| `crates/engine` | the engine exported to JavaScript: the ground (rasters over the relief), the layers (`render/`), the passes, picking, viewsheds, panoramas |

It ships as three WebAssembly modules, loaded only when needed:

| Module | Size (gzip) | Loaded |
|---|---|---|
| `isard_core_bg.wasm` | ~245 KB | first, by every map: the engine without 3D Tiles |
| `isard_bg.wasm` | ~415 KB | when a 3D Tiles layer, a model, a point cloud or panoramas are added (or `tiles3d: true`): the same engine with its `tiles3d` feature; the map moves to it and recreates its layers (`restore`) |
| `isard_points_bg.wasm` | ~43 KB | in the decoding workers, with the first COPC chunk: the LAZ decoder |

The engine holds no network code of its own beyond the raster downloads: it says what it wants (`wantedTiles`, the
3D Tiles and point cloud requests) and receives what arrives. Its state is plain Rust (the raster cache, the 3D Tiles
tree, styles, placement), kept apart from the WebGL calls so that it can be tested without a browser.

## The SDK

`packages/isard` is the npm package. `GeoMap` (`src/map.ts`) is what an application holds; its parts live in
`src/map/`:

| Part | Role |
|---|---|
| `engines.ts` | the engine class, the upgrade to the engine with 3D Tiles |
| `layer-stack.ts` | the drawing order of the layers, mirrored into the engine |
| `frame-loop.ts` | frames on demand: a frame is drawn only when something asked for it (a move, a tile, an animation) |
| `overlay-host.ts` | markers, popups, panels and controls positioned over the canvas after each frame |
| `hover-pick.ts` | what is under the pointer, read back a frame or two later without stalling the GPU |
| `workers.ts` | the shared pools of workers |

Each `add…` method returns a **layer handle** (`src/layers/`): a thin object holding the layer's id in the engine and
sending it its data and style. The handles survive an engine change: on the upgrade to 3D Tiles or after a lost WebGL
context, the map creates a new engine and every handle recreates its layer in it.

`isard/react` (`src/react/`) is a binding over the same handles: each component adds its layer or overlay when it
mounts, sends the least its props need when they change, and removes it when it unmounts. What a framework binding
needs to decide (which props a layer takes again, which ones make it anew, the order of the layers) is free of React in
`src/bind/`, so that another binding can reuse it.

## The workers

Two workers keep the heavy work off the main thread:

- **`basemap.worker.js`** draws the vector basemaps: it fetches and decodes the vector tiles (MVT, PMTiles), draws
  their fills, lines and circles with the MapLibre style into images (Canvas 2D), and lists the labels for the page
  to shape and place. It also decodes the relief's PNG tiles and draws the rasters reprojected from other grids
  (Lambert-93).
- **`decoder.worker.js`** decodes 3D Tiles content with the engine's own module (compiled once on the main thread and
  shared), Draco geometry and KTX2 textures with their decoders (loaded on first use), and the COPC chunks of point
  clouds with the point cloud module. It transfers GPU-ready arrays back: the main thread only uploads them.

Their messages are typed in `src/engine/protocol.ts`; the request headers of the page (`setRequestHeaders`) are sent
to them so that every request carries them.

## A frame

```
 a move, a tile arrived, an animation ──► requestRender()
                                              │ (one frame per animation frame at most)
 frame-loop ──► Engine.render()
                 ├─ camera settled on the relief; tiles in view selected (geo::tiles)
                 ├─ ground: rasters draped over the relief, the globe or the plane, the sky and atmosphere
                 ├─ layers in their order: features, extrusions, networks, 3D Tiles, models, point clouds, custom
                 ├─ effects: viewsheds (a depth cube per observer), swipe
                 └─ labels placed and drawn last
            ◄── the tiles to request, whether it is still loading
 frame-loop ──► overlays positioned, 'render' / 'move' / 'idle' events
```

Nothing is drawn while nothing changes: the map is idle once every tile in view has arrived and no animation runs
(`idle` event, `layer.ready`).

## A tile

- **Rasters** (basemaps, WMTS, XYZ, the relief): the engine downloads the images itself and decodes them with the
  browser (`createImageBitmap`); the tiles drawn by code (a vector basemap, a reprojected grid, your `loader`) come
  from the page and its basemap worker.
- **Vector basemaps**: the page asks the basemap worker for the tiles in view; it returns images for the raster pool
  and label candidates, which the page shapes (glyphs) and the engine places.
- **3D Tiles and point clouds**: the engine traverses its tree, then asks the tile loader (`src/engine/loader.ts`)
  for the content it wants, by priority; a decoding worker fetches and decodes it; the engine uploads what comes back
  and drops what fell out of view (budgets in GPU memory and points).

## Building and packaging

`bun run build` (`scripts/build.ts`) compiles the crates to the three modules, generates their bindings
(wasm-bindgen), optimises them (wasm-opt), compiles the package's TypeScript, bundles the two workers on their own, and
checks the gzipped sizes against their budgets (`scripts/size.ts`): a build over budget fails. The npm package
ships the modules, the workers and the ES modules side by side; `engine-loader.ts` finds them next to itself, at the
`assetsUrl` given, or where the [Vite plugin](./guide/getting-started#with-a-bundler) copied them.

The [develop guide](./guide/develop) has the commands, the tests (Rust natively, TypeScript with `bun test`, the examples
in a headless browser against recorded data) and the release.
