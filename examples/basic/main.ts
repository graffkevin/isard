/** A map: the vector PLAN.IGN basemap, the navigation control, a marker with its popup */
import { GeoMap, Marker, NavigationControl, Popup } from '@ign-junn/isard';
import { element } from '../shared/wfs.ts';

const IGN: [number, number] = [2.424573, 48.845726];

const map = await GeoMap.create(element('map'), {
  center: IGN,
  zoom: 15,
  basemap: 'plan-grey-vector',
  projection: 'mercator',
});
map.addControl(new NavigationControl({ visualizePitch: true }), 'top-right');

const popup = new Popup().setHTML('<b>IGN</b><br>73 avenue de Paris, Saint-Mandé');
const marker = new Marker({ draggable: true }).setLonLat(IGN).setPopup(popup).addTo(map);

// A click elsewhere moves the marker there, a short flight follows it
map.on('click', ({ lonLat }) => {
  if (!lonLat) return;
  marker.setLonLat(lonLat);
  map.easeTo({ center: lonLat }, 500);
});
