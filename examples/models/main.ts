/**
 * glTF models: a wind turbine (a small GLB next to the page) drawn at a row of positions on the IGN relief, one
 * instanced draw, each instance standing on the ground and facing the wind; Cesium's milk truck (Khronos glTF sample,
 * textured) from GitHub. Models load with the engine that draws the 3D Tiles; the instance under the cursor is
 * picked.
 */
import { GeoMap, type ModelInstance } from 'isard';
import { element } from '../shared/page.ts';

const map = await GeoMap.create(element('map'), {
  center: [2.8, 44.1852],
  zoom: 15.7,
  pitch: 62,
  bearing: 30,
  basemap: 'osm-liberty',
  terrain: { model: 'ign', exaggeration: 1.5, maxZoom: 14 },
  projection: 'mercator',
});

// A row of turbines across the plateau, 250 m apart, the rotors facing the south-west wind
const turbines: ModelInstance[] = Array.from({ length: 7 }, (_, i) => ({
  lonLat: [2.7955 + i * 0.0025, 44.183 + i * 0.0013],
  heading: 225,
  scale: 0.9 + 0.05 * (i % 3),
}));
const farm = map.addModels({ url: 'wind-turbine.glb', instances: turbines });

const trucks = map.addModels({
  url: 'https://raw.githubusercontent.com/KhronosGroup/glTF-Sample-Assets/main/Models/CesiumMilkTruck/glTF-Binary/CesiumMilkTruck.glb',
  instances: [0, 1, 2].map((i) => ({ lonLat: [2.7992 + i * 0.0003, 44.1846 + i * 0.00015], heading: 60, scale: 3 })),
});

map.hoverPicking = { layers: [farm, trucks], radius: 4 };
map.on('hoverfeature', ({ hit }) => {
  element('info').textContent = hit ? `${hit.layer === farm ? 'Turbine' : 'Truck'} ${hit.index + 1}` : ' ';
});
