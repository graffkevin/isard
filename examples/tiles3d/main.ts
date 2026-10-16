/** 3D Tiles: the Lille photomesh (Métropole Européenne de Lille, 2020, 5 cm, open data) over the orthophoto */
import { GeoMap } from '@ign-junn/isard';
import { element } from '../shared/wfs.ts';

const map = await GeoMap.create(element('map'), {
  center: [3.0635, 50.6366],
  zoom: 17,
  pitch: 55,
  bearing: 30,
  basemap: 'ortho',
  hillshade: 0,
  projection: 'mercator',
});

const mesh = map.addTiles3d({
  url: 'https://webimaging.lillemetropole.fr/externe/maillage/2020_mel_5cm/tileset.json',
  maxScreenSpaceError: 8,
});
map.on('render', () => {
  const s = mesh.stats();
  if (s)
    element('tiles').textContent = `${s.drawn} tiles drawn, ${s.inFlight} loading, ${s.gpuMb.toFixed(0)} MB on the GPU`;
});
