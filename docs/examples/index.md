# Examples

Each example is a page of [`examples/`](https://github.com/graffkevin/isard/tree/main/examples) in the repository: an
`index.html` and its `main.ts`, importing `isard` as an application does. The basemap is
OpenFreeMap's (OpenStreetMap data); the French examples take the IGN's data from the Géoplateforme, the Moon's and
Mars's come from NASA, no key needed.

| | |
|---|---|
| [The globe](./globe) | turning in its atmosphere, a flight to a click |
| [A map](./basic) | the vector basemap, controls, a draggable marker and its popup |
| [Travel times on hover](./network) | a network recolored at every move |
| [GeoJSON layers](./geojson) | fills by value, outlines, labels, picking |
| [Buildings](./buildings) | raised to their height, colored by it |
| [Relief and a ground shader](./relief) | your GLSL over the mountains |
| [3D Tiles](./tiles3d) | the Lille photomesh |
| [3D Tiles features](./tiles3d-features) | buildings picked with their properties, the layer tinted |
| [glTF models](./models) | wind turbines on the relief, textured trucks, picked |
| [Point clouds](./pointcloud) | Notre-Dame de Paris in the IGN's LiDAR HD, a COPC file read by range requests |
| [Viewshed](./viewshed) | what can be seen from a point over the relief and the buildings (prototype) |
| [Viewshed on a mesh](./viewshed-mesh) | the same over the textured mesh of Strasbourg (3D Tiles) |
| [Panoramas](./panoramas) | street-level images of Paris projected onto the buildings, a walk from one to the next |
| [Swipe](./swipe) | the orthophotos of 1950-1965 against today's, either side of a line |
| [3D panels](./panels) | elements of the page standing in 3D, raised markers |
| [three.js in the map](./threejs) | a custom layer sharing the depth of the buildings |
| [Vector basemaps](./vector) | OpenFreeMap's styles, a restyled subset |
| [Lambert-93 data](./lambert93) | PLAN IGN reprojected from Lambert-93, a WFS in EPSG:2154, a GPX file |
| [The Moon](./moon) | another body: the LRO mosaic reprojected over LOLA's relief, the Apollo sites, its craters in 3D |
| [Mars](./mars) | a body with an atmosphere: its dusty halo, the Viking mosaic over MOLA's relief, Olympus Mons in 3D |
