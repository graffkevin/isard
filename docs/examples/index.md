# Examples

Each example is a page of `examples/` in the repository: an
`index.html` and its `main.ts`, importing `isard` as an application does. Each one is also written
in React, with components ([React examples](../react/examples/)). The basemap is
OpenFreeMap's (OpenStreetMap data); the French examples take the IGN's data from the Géoplateforme, the Moon's and
Mars's come from NASA, no key needed.

| | |
|---|---|
| The globe | turning in its atmosphere, a flight to a click | [TypeScript](./globe) · [React](../react/examples/globe) |
| A map | the vector basemap, controls, a draggable marker and its popup | [TypeScript](./basic) · [React](../react/examples/basic) |
| Travel times on hover | a network recolored at every move | [TypeScript](./network) · [React](../react/examples/network) |
| GeoJSON layers | fills by value, outlines, labels, picking | [TypeScript](./geojson) · [React](../react/examples/geojson) |
| Buildings | raised to their height, colored by it | [TypeScript](./buildings) · [React](../react/examples/buildings) |
| Relief and a ground shader | your GLSL over the mountains | [TypeScript](./relief) · [React](../react/examples/relief) |
| 3D Tiles | the Lille photomesh | [TypeScript](./tiles3d) · [React](../react/examples/tiles3d) |
| 3D Tiles features | buildings picked with their properties, the layer tinted | [TypeScript](./tiles3d-features) · [React](../react/examples/tiles3d-features) |
| glTF models | wind turbines on the relief, textured trucks, picked | [TypeScript](./models) · [React](../react/examples/models) |
| Point clouds | Notre-Dame de Paris in the IGN's LiDAR HD, a COPC file read by range requests | [TypeScript](./pointcloud) · [React](../react/examples/pointcloud) |
| Viewshed | what can be seen from a point over the relief and the buildings (prototype) | [TypeScript](./viewshed) · [React](../react/examples/viewshed) |
| Viewshed on a mesh | the same over the textured mesh of Strasbourg (3D Tiles) | [TypeScript](./viewshed-mesh) · [React](../react/examples/viewshed-mesh) |
| Panoramas | street-level images of Paris projected onto the buildings, a walk from one to the next | [TypeScript](./panoramas) · [React](../react/examples/panoramas) |
| Swipe | the orthophotos of 1950-1965 against today's, either side of a line | [TypeScript](./swipe) · [React](../react/examples/swipe) |
| 3D panels | elements of the page standing in 3D, raised markers | [TypeScript](./panels) · [React](../react/examples/panels) |
| three.js in the map | a custom layer sharing the depth of the buildings | [TypeScript](./threejs) · [React](../react/examples/threejs) |
| Vector basemaps | OpenFreeMap's styles, a restyled subset | [TypeScript](./vector) · [React](../react/examples/vector) |
| Lambert-93 data | PLAN IGN reprojected from Lambert-93, a WFS in EPSG:2154, a GPX file | [TypeScript](./lambert93) · [React](../react/examples/lambert93) |
| The Moon | another body: the LRO mosaic reprojected over LOLA's relief, the Apollo sites, its craters in 3D | [TypeScript](./moon) · [React](../react/examples/moon) |
| Mars | a body with an atmosphere: its dusty halo, the Viking mosaic over MOLA's relief, Olympus Mons in 3D | [TypeScript](./mars) · [React](../react/examples/mars) |
