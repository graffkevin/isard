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
| [Custom layers](./custom) | `addCustomLayer` | your WebGL code or a three.js scene, in the map's frame and depth |

Every `add…` returns a handle: `layer.setStyle(…)`, `layer.setVisible(b)`, `layer.moveBefore(other)`,
`layer.remove()`, and what is specific to the layer (`setValues`, `setData`…). Each takes `before` to go under
another layer.
