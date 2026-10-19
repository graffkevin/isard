/**
 * Viewshed: what can be seen from a point. From the forecourt of the Sacré-Cœur on the hill of Montmartre, over the
 * IGN's relief (RGE ALTI) and the BD TOPO buildings raised to their height: the scene is drawn from the observer into
 * a depth cube, and each pixel on screen tinted by whether something stands between it and the observer. A click
 * moves the observer; its height above the ground and the range follow the sliders.
 */
import { GeoMap, geoplateformeWfs, type LonLat } from 'isard';
import { element } from '../shared/page.ts';

type Building = { hauteur: number | null };

const map = await GeoMap.create(element('map'), {
  center: [2.3435, 48.8848],
  zoom: 15.6,
  pitch: 55,
  bearing: 160,
  basemap: 'ign-plan-grey-vector',
  terrain: { model: 'ign' },
  projection: 'mercator',
});

const buildings = await geoplateformeWfs<Building>('BDTOPO_V3:batiment', [2.332, 48.877, 2.356, 48.893]);
map.addExtrusions({ data: buildings, height: (p) => p.hauteur ?? 6, color: '#e4e1d8' });

let observer: LonLat = [2.34305, 48.88615];
const height = element<HTMLInputElement>('height');
const range = element<HTMLInputElement>('range');

const show = () => {
  element('height-value').textContent = `${height.value} m`;
  element('range-value').textContent = `${range.value} m`;
  map.setViewshed({ position: observer, height: Number(height.value), range: Number(range.value) });
};
height.addEventListener('input', show);
range.addEventListener('input', show);
map.on('click', ({ lonLat }) => {
  if (!lonLat) return;
  observer = lonLat;
  show();
});
show();
