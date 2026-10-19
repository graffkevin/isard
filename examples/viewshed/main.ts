/**
 * Viewshed: what can be seen from a point. From the forecourt of the Sacré-Cœur on the hill of Montmartre, over the
 * IGN's relief (RGE ALTI) and the BD TOPO buildings raised to their height: the scene is drawn from the observer into
 * a depth cube, and each pixel on screen tinted by whether something stands between it and the observer. The pin is
 * the observer: drag it, or click elsewhere; its height above the ground and the range follow the sliders.
 */
import { GeoMap, geoplateformeWfs, type LonLat, Marker } from 'isard';
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
const pin = new Marker({ draggable: true }).setLonLat(observer).addTo(map);
const height = element<HTMLInputElement>('height');
const range = element<HTMLInputElement>('range');
const oneWay = element<HTMLInputElement>('one-way');
const direction = element<HTMLInputElement>('direction');
const fov = element<HTMLInputElement>('fov');

// The edges of the cone on the ground, from the observer to the range
const edges = map.addLines({ data: [], color: '#1c7ed6', width: 2.5 });
const along = ([lon, lat]: LonLat, heading: number, metres: number): LonLat => {
  const b = (heading * Math.PI) / 180;
  const k = metres / 6378137;
  return [
    lon + ((k * Math.sin(b)) / Math.cos((lat * Math.PI) / 180)) * (180 / Math.PI),
    lat + k * Math.cos(b) * (180 / Math.PI),
  ];
};

const show = () => {
  element('height-value').textContent = `${height.value} m`;
  element('range-value').textContent = `${range.value} m`;
  element('direction-value').textContent = `${direction.value}°`;
  element('fov-value').textContent = `${fov.value}°`;
  const cone = oneWay.checked ? { direction: Number(direction.value), fov: Number(fov.value) } : {};
  map.setViewshed({ position: observer, height: Number(height.value), range: Number(range.value), ...cone });
  const d = Number(range.value);
  const [a, b] = [-1, 1].map((side) => along(observer, Number(direction.value) + (side * Number(fov.value)) / 2, d));
  edges.setData(
    oneWay.checked && a && b
      ? [{ type: 'Feature', properties: {}, geometry: { type: 'LineString', coordinates: [a, observer, b] } }]
      : [],
  );
};
for (const input of [height, range, oneWay, direction, fov]) input.addEventListener('input', show);
// The view follows the pin as it is dragged
pin.on('drag', ({ lonLat }) => {
  observer = lonLat;
  show();
});
map.on('click', ({ lonLat }) => {
  if (!lonLat) return;
  observer = lonLat;
  pin.setLonLat(lonLat);
  show();
});
show();
