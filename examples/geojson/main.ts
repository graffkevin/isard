/**
 * GeoJSON layers: the communes around Paris (the IGN's ADMIN EXPRESS, from the Géoplateforme's WFS) as fills colored by their population through a ramp,
 * their outlines and their names; the commune under the cursor outlined and described.
 */
import { GeoMap, geoplateformeWfs, IGN_ATTRIBUTION } from 'isard';
import { element } from '../shared/page.ts';

type Commune = { nom_officiel: string; population: number; code_insee: string };

const map = await GeoMap.create(element('map'), {
  center: [2.39, 48.84],
  zoom: 10.6,
  projection: 'mercator',
  // The communes are the IGN's data
  attributionControl: { customAttribution: IGN_ATTRIBUTION },
});

const communes = await geoplateformeWfs<Commune>('ADMINEXPRESS-COG.LATEST:commune', [2.15, 48.72, 2.62, 48.95]);
const fills = map.addFills({
  data: communes,
  color: '#999',
  opacity: 0.55,
  ramp: [
    [0, '#f7fbff'],
    [20_000, '#9ecae1'],
    [60_000, '#3182bd'],
    [150_000, '#08306b'],
  ],
});
// One value per feature, colored by the ramp: one texture upload
fills.setValues(Float32Array.from(communes.features, (f) => f.properties.population));
map.addLines({
  data: communes,
  color: '#fff',
  width: [
    [9, 0.5],
    [14, 2],
  ],
});
const hovered = map.addLines({ data: communes, color: '#e8590c', width: 3, filter: () => false });
map.addSymbols({
  data: communes,
  text: (p) => p.nom_officiel,
  size: [
    [10, 10],
    [14, 14],
  ],
  color: '#1d2433',
  haloColor: '#fff',
  haloWidth: 1.5,
  minZoom: 10.5,
});

// Picked without stalling the GPU, a frame after the pointer moved
map.hoverPicking = { layers: [fills] };
map.on('hoverfeature', ({ hit }) => {
  const index = hit?.index;

  hovered.setFilter((_, i) => i === index);

  const p = hit && fills.featureAt(hit.index)?.properties;

  element('info').textContent = p ? `${p.nom_officiel}: ${p.population.toLocaleString('en')} inhabitants` : ' ';
});
