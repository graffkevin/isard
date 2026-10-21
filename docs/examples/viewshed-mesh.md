# Viewshed on a mesh

What can be seen from a point over a textured mesh: the photomesh of the Eurométropole de Strasbourg (2022, open data) in 3D Tiles, on the IGN relief, around the cathedral. The mesh's walls and roofs are drawn into each observer's depth cube as the relief is: they hide what lies behind them, and are tinted seen (green) or hidden (red). Only the tiles the map loaded for its view hide anything, at the detail chosen for the camera. As in the [viewshed example](./viewshed): a click on the map adds an observer where the click meets the mesh, on a roof or the spire as on the ground (`map.pickSurface`), a click on a pin opens its settings, and the pin drags over the mesh.

The same in [React](./react/viewshed-mesh).

<LiveExample slug="viewshed-mesh" height="560px" />

<<< @/../examples/viewshed-mesh/main.ts
