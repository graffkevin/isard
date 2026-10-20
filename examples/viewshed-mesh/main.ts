/**
 * Viewshed over a textured mesh: the photomesh of the Eurométropole de Strasbourg (2022, open data) in 3D Tiles, on
 * the IGN relief (RGE ALTI), around the cathedral. The mesh's surfaces are drawn into each observer's depth cube as the
 * relief is: its walls and roofs hide what lies behind them, and are tinted seen or hidden. Only the tiles the map
 * loaded for its view hide anything, at the detail chosen for the camera. A click on the map adds an observer where
 * the click meets the mesh, on a roof as on the ground; a click on its pin opens its settings (one popup open at a
 * time), and the pin drags over the mesh.
 */
import { GeoMap } from 'isard';
import { observers } from '../shared/observers.ts';
import { element } from '../shared/page.ts';

const map = await GeoMap.create(element('map'), {
  center: [7.7505, 48.5817],
  zoom: 17,
  pitch: 60,
  bearing: 70,
  basemap: 'ign-ortho',
  // The mesh's heights are altitudes (NGF-IGN69), not heights above the ellipsoid: the relief as served, under it
  terrain: { model: 'ign', geoid: false },
  projection: 'mercator',
  // 3D Tiles at once: the engine that draws them from the start, not after the one without them
  tiles3d: true,
});

map.addTiles3d({
  url: 'https://s3.eu-west-2.wasabisys.com/ems-sgct-photomaillage/ODACIT/EMS_PM2022/tileset.json',
  maxScreenSpaceError: 8,
});

// On the cathedral's square, looking along its front
observers(map, { surfaces: true, defaults: { range: 400, direction: 60, fov: 90 } })([7.7499, 48.5816]);
