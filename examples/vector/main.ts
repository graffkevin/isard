/**
 * Vector basemaps: OpenFreeMap's styles drawn from its vector tiles (OpenStreetMap data, the OpenMapTiles schema), and
 * a subset of a style on its own raster layer, restyled: the rivers and water areas in a strong blue, over the basemap
 * (they could go over a flood layer of the application).
 */
import { type Basemap, GeoMap } from 'isard';
import { element } from '../shared/page.ts';

const map = await GeoMap.create(element('map'), {
  center: [1.909, 47.902],
  zoom: 13,
  basemap: 'osm-positron',
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
      'source-layer': 'water',
      paint: { 'fill-color': '#2f7fc4', 'fill-opacity': 0.55 },
    },
    {
      id: 'rivers',
      type: 'line',
      'source-layer': 'waterway',
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

const styles = ['positron', 'liberty', 'bright'];
for (const id of styles) {
  element(id).onclick = () => {
    map.setBasemap(`osm-${id}` as Basemap);
    for (const other of styles) element(other).classList.toggle('on', other === id);
  };
}
element<HTMLInputElement>('rivers').onchange = (e) => rivers.setStyle((e.target as HTMLInputElement).checked ? 1 : 0);
