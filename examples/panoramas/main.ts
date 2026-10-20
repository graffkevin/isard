/**
 * Panoramas: the oriented images of a vehicle's cameras (iTowns' immersive sample of Paris, rue Servandoni) projected
 * onto the scene around the panorama shown, the BD TOPO buildings raised to take them. From above, the panorama
 * colors its street; walking in puts the eye at the cameras: the drag turns it, a click on the street goes to the
 * panorama nearest to the point clicked, Z and S to the next and the previous ones.
 */
import { GeoMap, geoplateformeWfs } from 'isard';
import { element } from '../shared/page.ts';

type Building = { hauteur: number | null };

const SAMPLE = 'https://raw.githubusercontent.com/iTowns/iTowns2-sample-data/master/immersive/exampleParis1';

const map = await GeoMap.create(element('map'), {
  center: [2.3346, 48.8505],
  zoom: 18.6,
  pitch: 60,
  bearing: 176,
  basemap: 'ign-plan-grey-vector',
  terrain: { model: 'ign' },
  projection: 'mercator',
});

const buildings = await geoplateformeWfs<Building>('BDTOPO_V3:batiment', [2.331, 48.847, 2.338, 48.853]);
map.addExtrusions({ data: buildings, height: (p) => p.hauteur ?? 10, color: '#e4e1d8' });

const panoramas = await map.addPanoramas({
  url: 'https://www.itowns-project.org/itowns-sample-data-small/images/140616/Paris-140616_0740-{cameraId}-00001_0000{panoId}.jpg',
  orientationsUrl: `${SAMPLE}/panoramicsMetaDataParis.geojson`,
  calibrationUrl: `${SAMPLE}/cameraCalibration.json`,
  backgroundDistance: 1200,
});
await panoramas.show(0);

const walk = element<HTMLButtonElement>('walk');
let walking = false;
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
