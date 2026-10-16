/**
 * Buildings: BD TOPO buildings raised to their height and colored by it through a ramp, with depth; the building under
 * the cursor described. A tilted view, the relief off.
 */
import { GeoMap } from '@ign-junn/isard';
import { element, wfs } from '../shared/wfs.ts';

type Building = { hauteur: number | null; usage_1: string | null; nombre_de_logements: number | null };

const map = await GeoMap.create(element('map'), {
  center: [2.4225, 48.8452],
  zoom: 16.4,
  pitch: 60,
  bearing: -25,
  basemap: 'plan-grey-vector',
  projection: 'mercator',
});

const buildings = await wfs<Building>('BDTOPO_V3:batiment', [2.414, 48.84, 2.432, 48.851]);
const layer = map.addExtrusions({
  data: buildings,
  height: (p) => p.hauteur ?? 6,
  color: '#d9d6cc',
  ramp: [
    [0, '#fde0c5'],
    [12, '#f59e72'],
    [25, '#d0587e'],
    [45, '#6c2167'],
  ],
});
layer.setValues(Float32Array.from(buildings.features, (f) => f.properties.hauteur ?? Number.NaN));

map.on('hover', ({ point }) => {
  const p = map.pickFeature(point, { layers: [layer] })?.feature?.properties as Building | undefined;
  element('info').textContent = p
    ? `${p.usage_1 ?? 'Building'}, ${p.hauteur ?? '?'} m${p.nombre_de_logements ? `, ${p.nombre_de_logements} homes` : ''}`
    : ' ';
});
