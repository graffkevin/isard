# glTF models

```ts
const turbines = map.addModels({
  url: 'wind-turbine.glb', // or data: an ArrayBuffer of the file
  instances: [
    { lonLat: [2.7955, 44.183], heading: 225 },
    { lonLat: [2.798, 44.1843], heading: 225, scale: 1.1 },
  ],
});
turbines.setInstances(moved); // instances moved, added, removed: the file stays on the GPU
map.pickFeature(point, { layers: [turbines] }); // { layer: turbines, index, feature: undefined }
```

A glTF binary (`.glb`) drawn at positions: its meshes uploaded once, every instance drawn in one instanced draw per
primitive, with depth (the relief and the buildings hide them, they hide what is behind them). The file is decoded as
3D Tiles content is, by the same decoders: meshopt and mesh quantization in Rust, Draco geometry and KTX2 textures in
the decoding workers (see [3D Tiles](./tiles3d#the-engine-with-3d-tiles) for the decoders and the Content Security
Policy). Its scene's node transforms apply; skins, morph targets and animations do not.

Each instance:

| | |
|---|---|
| `lonLat` | where its origin stands |
| `altitude` | metres above the ground (the relief under it, as drawn), 0 by default; above the ellipsoid with `altitudeReference: 'sea'` |
| `heading` | degrees clockwise from the north of the model's front (glTF's +Z; its up is +Y) |
| `pitch`, `roll` | degrees, nose up, right side down |
| `scale` | 1 by default: the file's metres |

Each instance's matrix is made every frame in double precision relative to the camera: exact at any zoom, on the plane
and the globe (where instances past the horizon are left out). Over the relief, the ground under an instance is the
copy of the relief kept on the CPU (32 × 32 altitudes per tile): on steep slopes a small model may stand a little in or
above the ground drawn; a finer relief (its `maxZoom`) makes it closer.

Textured surfaces are drawn unlit, as the 3D Tiles are, their texture times their material's base color factor;
surfaces without texture take that base color, lit from above so that their shape reads.

Like the 3D Tiles, models are drawn by the engine with 3D Tiles: the first model on a map loads it (`tiles3d: true`
to start with it, `'preload'` to load it ahead, see [3D Tiles](./tiles3d#the-engine-with-3d-tiles)); the layer shows
once it is there. A file that cannot be loaded is reported in the console and the layer stays empty.

<LiveExample slug="models" height="480px" />

For a scene of your own (three.js, animated objects, your shaders), see [custom layers](./custom).
