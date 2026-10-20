/**
 * 3D panels: elements of the page standing in 3D (`Panel`, any element: a video, an iframe, a page): a photo 40
 * metres wide on the forecourt of the Sacré-Cœur and a sign below it, both facing the camera's first view; markers raised 30 and 60 metres above the
 * ground, their stems down to it
 * (`Marker`'s `height`). Placed after each frame by a CSS 3D transform from the camera.
 */
import { GeoMap, Marker, Panel } from 'isard';
import { element } from '../shared/page.ts';

const map = await GeoMap.create(element('map'), {
  center: [2.3431, 48.8858],
  zoom: 17.2,
  pitch: 62,
  bearing: 160,
  basemap: 'ign-plan-grey-vector',
  terrain: { model: 'ign' },
  projection: 'mercator',
});

const screen = document.createElement('div');
screen.className = 'screen';
// Any element stands in 3D: a video, an iframe, a page; here a photo of a street of Paris
const photo = document.createElement('img');
photo.src =
  'https://www.itowns-project.org/itowns-sample-data-small/images/140616/Paris-140616_0740-300-00001_0000482.jpg';
photo.alt = 'A street of Paris';
screen.append(photo);
new Panel({ element: screen, width: 40, heading: 340, height: 4 }).setLonLat([2.3431, 48.8861]).addTo(map);

const sign = document.createElement('div');
sign.className = 'sign';
sign.textContent = 'Square Louise-Michel';
new Panel({ element: sign, width: 30, heading: 340, height: 2 }).setLonLat([2.3433, 48.8848]).addTo(map);

for (const [lonLat, height] of [
  [[2.3418, 48.8859], 30],
  [[2.3446, 48.8857], 60],
] as const) {
  new Marker({ height }).setLonLat([...lonLat]).addTo(map);
}
