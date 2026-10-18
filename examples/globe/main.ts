/**
 * The globe: the IGN orthophotos (Géoplateforme; worldwide imagery at low zoom) on a sphere among the stars, turning slowly until the map is touched; a click flies to
 * the point (van Wijk and Nuij's path, as MapLibre's flyTo), the sphere blending into Web Mercator on the way.
 */
import { GeoMap } from 'isard';
import { element } from '../shared/page.ts';

const map = await GeoMap.create(element('map'), {
  center: [2.5, 30],
  zoom: 1.4,
  projection: 'globe',
  basemap: 'ign-ortho',
});

// Turns a few degrees a second until the first gesture
let turning = true;
let last = performance.now();
const turn = (now: number) => {
  if (!turning) return;
  const { center } = map.getCamera();
  map.setCamera({ center: [center[0] + ((now - last) / 1000) * 4, center[1]] });
  last = now;
  requestAnimationFrame(turn);
};
requestAnimationFrame(turn);
map.getCanvas().addEventListener('pointerdown', () => {
  turning = false;
});
map.on('click', ({ lonLat }) => {
  turning = false;
  if (lonLat) map.flyTo({ center: lonLat, zoom: 12, pitch: 45 }, { duration: 5000 });
});
