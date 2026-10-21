/**
 * 3D Tiles features: a city of blocks from the 3D Tiles samples (Cesium's, b3dm with batch tables) picked one by
 * one, each with its properties (`pickFeature` on a 3D Tiles layer: the batch table of 3D Tiles 1.0, the property
 * table of `EXT_structural_metadata` in 1.1); the layer tinted by a color (`setStyle({ color })`).
 */
import { GeoMap, Popup } from 'isard';
import { element } from '../shared/page.ts';

const map = await GeoMap.create(element('map'), {
  center: [-75.6123, 40.0425],
  zoom: 17.6,
  pitch: 55,
  bearing: 30,
  basemap: 'osm-positron',
  projection: 'mercator',
  tiles3d: true,
});
const color = element<HTMLInputElement>('color');
const city = map.addTiles3d({
  url: 'https://raw.githubusercontent.com/CesiumGS/3d-tiles-samples/main/1.0/TilesetWithRequestVolume/city/tileset.json',
  color: color.value,
});
color.addEventListener('input', () => city.setStyle({ color: color.value }));

const popup = new Popup({ maxWidth: '260px' });
map.on('click', ({ point, lonLat }) => {
  const hit = map.pickFeature(point, { layers: [city] });

  if (!hit?.feature || !lonLat) {
    popup.remove();

    return;
  }

  const table = document.createElement('table');

  Object.entries(hit.feature.properties ?? {}).forEach(([name, value]) => {
    const row = table.insertRow();

    row.insertCell().textContent = name;
    row.insertCell().textContent = typeof value === 'number' ? String(Math.round(value * 1e6) / 1e6) : String(value);
  });
  popup.setLonLat(lonLat).setDOMContent(table).addTo(map);
});
