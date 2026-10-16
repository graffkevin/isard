# 3D Tiles

```ts
const mesh = map.addTiles3d({ url: 'https://…/tileset.json', maxScreenSpaceError: 8 });
mesh.stats(); // { drawn, loaded, inFlight, prefetching, gpuMb, tiles }
```

3D Tiles 1.0 (b3dm) and 1.1 (glb), external tilesets, compressed or not: meshopt and mesh quantization decoded in Rust,
Draco geometry and KTX2 (Basis Universal) textures decoded in workers by the official modules, loaded on first use;
KTX2 textures stay compressed on the GPU (ASTC, BC7, ETC2 or S3TC). Traversal by screen-space error, children
replacing their parent only once all are ready, downloads aborted when they leave the view, a GPU memory budget.

With the relief on, its altitudes get the geoid height so that the meshes (ellipsoidal heights) sit on the ground.
The decoders come from gstatic.com and jsDelivr by default: `decoders` to self-host them.

<LiveExample slug="tiles3d" height="440px" />
