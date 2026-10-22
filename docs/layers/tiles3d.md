# 3D Tiles

```ts
const mesh = map.addTiles3d({ url: 'https://…/tileset.json', maxScreenSpaceError: 8 });
mesh.stats(); // { drawn, loaded, inFlight, prefetching, gpuMb, tiles }
```

3D Tiles 1.0 (b3dm, and pnts point clouds) and 1.1 (glb, triangles and points), external tilesets, implicit tiling (quadtree and octree, binary or JSON subtrees,
box or region volumes), several contents per tile, compressed or not: meshopt and mesh quantization decoded in Rust,
Draco geometry and KTX2 (Basis Universal) textures decoded in workers by the official modules, loaded on first use;
KTX2 textures stay compressed on the GPU (ASTC, BC7, ETC2 or S3TC). Traversal by screen-space error, children
replacing their parent only once all are ready, downloads aborted when they leave the view, a GPU memory budget.

## Picking and properties

A 3D Tiles layer is picked like the others: `pickFeature` (or the `hoverfeature` event) gives the feature under the
point and its properties, from its tile's `EXT_mesh_features` feature IDs and `EXT_structural_metadata` property
table (3D Tiles 1.1: scalars, booleans, strings, enums), or a b3dm's `_BATCHID` and batch table (3D Tiles 1.0, its
JSON arrays). The IGN's Bâtiments 3D answer with their BD TOPO attributes:

```ts
const buildings = map.addTiles3d({ url: 'https://…/tileset.json', geoid: true });
map.on('click', ({ point }) => {
  const hit = map.pickFeature(point, { layers: [buildings] });
  console.log(hit?.feature?.properties); // { cleabs, hauteur, nature, usage_1, nombre_d_etages, … }
});
```

`hit.feature.id` is the feature's index in its tile; a mesh without features (a photomesh) gives its tile, without
properties. See [3D Tiles features](../examples/tiles3d-features).

## Color and heights

`color` (in `addTiles3d` and `setStyle`) tints the textures of the meshes and colors the untextured ones (light grey
without it); untextured meshes are shaded by the slope of their faces to a light from the north-west, textured ones
keep the light of their photos.

3D Tiles are specified in heights above the ellipsoid, as the photomeshes are (over France, `terrain: 'ign'` raises
the IGN relief by the RAF20 geoid so that they sit on it). A tileset in altitudes (NGF-IGN69), as the IGN's
Bâtiments 3D, takes `geoid: true`: each tile raised by the geoid onto the relief (44 m under it in Paris without).

## The engine with 3D Tiles

The 3D Tiles code is a third of the engine: maps load the engine without it, and the first 3D Tiles layer (or
[model](./models), which shares its decoders, or [point cloud](./pointclouds)) loads the full one (the layer shows once it is there, `stats()` is
undefined until then). The map then moves onto it: a
new engine on the same canvas, the camera, settings and every layer created again in it, in order, with the same
handles. That costs a download (some 416 KB gzipped, compiled off the main thread) and, once it is there, a stall
of a frame or two while the map is rebuilt (shaders, the layers' buffers, the raster tiles fetched again): the
view may hold or flash once. The rebuild runs in a task of its own, after the one that instantiated the engine.

- `tiles3d: true` loads the full engine from the start, for a map that shows 3D Tiles at once: one engine
  downloaded instead of two, no move.
- `tiles3d: 'preload'` starts on the engine without 3D Tiles and downloads and compiles the full one once the map
  is idle, without running it: the first 3D Tiles layer does not wait for the download, only for the rebuild. For a
  map that offers 3D Tiles later, on demand.
- A map that never adds a 3D Tiles layer never downloads the full engine (`false`, the default).

A failed download is reported (the console shows it as an uncaught error); the 3D Tiles layers stay empty and the
next one added tries again.

Meshes have ellipsoidal heights: over France, the IGN's relief (`terrain: 'ign'`) gets the RAF20 geoid height so
that they sit on the ground (`geoid: true` does it for another model).
The decoders come from gstatic.com and jsDelivr by default: `decoders` to self-host them.

::: warning Content Security Policy
The Draco and Basis modules are Emscripten scripts that the decoding worker fetches and evaluates (module workers
have no `importScripts`): with a CSP, the worker needs `script-src 'unsafe-eval'` and `connect-src` to their folder.
Self-hosting them is the way to depend on no third party at run time:

```ts
await GeoMap.create(container, { decoders: { draco: '/decoders/draco/', basis: '/decoders/basis/' } });
```

Copy `draco_wasm_wrapper_gltf.js` and `draco_decoder_gltf.wasm` (from the `draco3d` package or gstatic; for Draco
point clouds, the full decoder `draco_wasm_wrapper.js` and `draco_decoder.wasm` too), and
`basis_transcoder.js` and `basis_transcoder.wasm` (from `three/examples/jsm/libs/basis/`), into those folders. A
tileset without Draco nor KTX2 loads neither.
:::

<LiveExample slug="tiles3d" height="440px" />
