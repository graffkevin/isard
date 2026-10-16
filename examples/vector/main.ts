/**
 * Vector basemaps: the PLAN.IGN styles drawn from the Géoplateforme's vector tiles, and a subset of a style on its own
 * raster layer, restyled: the rivers and water areas in a strong blue, over the basemap (they could go over a flood
 * layer of the application).
 */
import { type Basemap, GeoMap } from '@ign-junn/isard';
import { element } from '../shared/wfs.ts';

const map = await GeoMap.create(element('map'), {
  center: [1.909, 47.902],
  zoom: 13,
  basemap: 'plan-grey-vector',
  projection: 'mercator',
});

// The rivers of the basemap's data, drawn by their own layers (the style's source and its source layers)
const tiles = map.vectorTiles();
const rivers = tiles.raster({
  layers: () => false,
  extraLayers: [
    {
      id: 'water',
      type: 'fill',
      'source-layer': 'hydro_surf',
      paint: { 'fill-color': '#2f7fc4', 'fill-opacity': 0.55 },
    },
    {
      id: 'rivers',
      type: 'line',
      'source-layer': 'hydro_reseau',
      layout: { 'line-cap': 'round', 'line-join': 'round' },
      paint: {
        'line-color': '#2f7fc4',
        'line-width': {
          stops: [
            [10, 1],
            [16, 5],
          ],
        },
      },
    },
  ],
});

for (const [id, basemap] of [
  ['grey', 'plan-grey-vector'],
  ['standard', 'plan-vector'],
] as [string, Basemap][]) {
  element(id).onclick = () => {
    map.setBasemap(basemap, 0.3);
    for (const other of ['grey', 'standard']) element(other).classList.toggle('on', other === id);
  };
}
element<HTMLInputElement>('rivers').onchange = (e) => rivers.setStyle((e.target as HTMLInputElement).checked ? 1 : 0);
