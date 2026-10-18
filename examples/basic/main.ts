/** A map: the default basemap (OpenFreeMap's Positron, OpenStreetMap data), the navigation control, a marker with its
 * popup */
import { GeoMap, Marker, NavigationControl, Popup } from 'isard';
import { element } from '../shared/page.ts';

const VINCENNES: [number, number] = [2.4358, 48.8427];

const map = await GeoMap.create(element('map'), {
  center: VINCENNES,
  zoom: 15,
  projection: 'mercator',
});
map.addControl(new NavigationControl({ visualizePitch: true }), 'top-right');

const popup = new Popup().setHTML('<b>Château de Vincennes</b><br>Drag the marker, or click the map');
const marker = new Marker({ draggable: true }).setLonLat(VINCENNES).setPopup(popup).addTo(map);

// A click elsewhere moves the marker there, a short flight follows it
map.on('click', ({ lonLat }) => {
  if (!lonLat) return;
  marker.setLonLat(lonLat);
  map.easeTo({ center: lonLat }, 500);
});
