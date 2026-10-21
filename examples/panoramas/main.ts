/**
 * Panoramas: the oriented images of a vehicle's cameras (iTowns' immersive sample of Paris, rue Servandoni) projected
 * onto the scene around the panorama shown, the BD TOPO buildings raised to take them. From above, the panorama
 * colors its street; the page starts walking in, the eye at the cameras: the drag turns it, the wheel goes to the
 * nearest panorama ahead (or behind), a click on the street to the one nearest to the point clicked, Z and S to the
 * next and the previous ones.
 */
import { bdtopoBuildings, GeoMap } from 'isard';
import { element } from '../shared/page.ts';

const SAMPLE = 'https://raw.githubusercontent.com/iTowns/iTowns2-sample-data/master/immersive/exampleParis1';

const map = await GeoMap.create(element('map'), {
  center: [2.3346, 48.8505],
  zoom: 18.6,
  pitch: 60,
  bearing: 176,
  preset: 'ign-3d',
});
map.addExtrusions(await bdtopoBuildings([2.331, 48.847, 2.338, 48.853], { defaultHeight: 10 }));

const panoramas = await map.addPanoramas({
  url: 'https://www.itowns-project.org/itowns-sample-data-small/images/140616/Paris-140616_0740-{cameraId}-00001_0000{panoId}.jpg',
  orientationsUrl: `${SAMPLE}/panoramicsMetaDataParis.geojson`,
  calibrationUrl: `${SAMPLE}/cameraCalibration.json`,
  backgroundDistance: 1200,
});
// Straight in the street, at the first panorama
await panoramas.enter({ index: 0 });

const walk = element<HTMLButtonElement>('walk');
let walking = true;
walk.textContent = 'Leave';
const where = () => {
  const pano = panoramas.current;
  element('where').textContent = pano ? `panorama ${pano.index + 1} of ${panoramas.list.length}` : '';
};
walk.addEventListener('click', async () => {
  walking = !walking;
  walk.textContent = walking ? 'Leave' : 'Walk in';
  if (walking) await panoramas.enter({ index: panoramas.current?.index ?? 0 });
  else {
    panoramas.exit();
    await panoramas.show(0);
  }
  where();
});
map.on('click', () => setTimeout(where, 0));
map.getContainer().addEventListener('keyup', where);
where();
