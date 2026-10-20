# Layers

| Layer | Method | For |
|---|---|---|
| [Lines, circles, fills](./features) | `addLines`, `addCircles`, `addFills` | GeoJSON: outlines, routes, stations, areas, veils |
| [Labels](./labels) | `addSymbols` | text at points, along lines, in polygons |
| [Networks](./networks) | `addNetwork` | millions of edges colored by a value per node: travel times, flows |
| [Buildings](./buildings) | `addExtrusions` | polygons raised into blocks, colored by values or by a network |
| [Ground shaders](./ground) | `addGround` | your GLSL over the ground: water depth, heat maps |
| [Rasters](./rasters) | `addRaster` | XYZ / WMTS tiles, or tiles drawn by your code |
| [3D Tiles](./tiles3d) | `addTiles3d` | photomeshes, city models |
| [glTF models](./models) | `addModels` | a glTF model at many positions: wind turbines, vehicles, street furniture |
| [Point clouds](./pointclouds) | `addPointCloud` | LiDAR in COPC (IGN's LiDAR HD), 3D Tiles points |
| [Custom layers](./custom) | `addCustomLayer` | your WebGL code or a three.js scene, in the map's frame and depth |

Every `add…` returns a handle: `layer.setStyle(…)`, `layer.setVisible(b)`, `layer.moveBefore(other)`,
`layer.remove()`, and what is specific to the layer (`setValues`, `setData`…). Each takes `before` to go under
another layer.

## Comparing two layers: swipe

```ts
map.setSwipe({ position: 400, layers: [old] }); // `old` left of x = 400 CSS pixels only
map.setSwipe({ position: 400, layers: [buildings], side: 'right' });
map.setSwipe(undefined);
```

The layers given show on one side of a vertical line only, the layers beneath them on the other side: two states of
a place, two basemaps. Rasters are cut in the ground's pass, the other layers by a scissor; set it again as the line
moves (a slider, a drag). See [Swipe](../examples/swipe).
