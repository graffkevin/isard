/**
 * Demo of the engine: Géoplateforme basemaps on the globe or in Web Mercator, with the tile counters of each layer
 * to watch loading while zooming. ?projection=mercator starts flat.
 */
import { type Basemap, GeoMap, type LonLat, type Projection, type Tiles3dLayer } from '../js/src/index.ts';

const IGN: LonLat = [2.424573, 48.845726];
const params = new URLSearchParams(location.search);
let projection: Projection = params.get('projection') === 'mercator' ? 'mercator' : 'globe';

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
// ?workers=0: 3D Tiles decoded on the main thread, to compare
const workers = params.get('workers') === '0' ? false : undefined;
const map = await GeoMap.create($('map'), { center: [2.5, 46.6], zoom: 2.2, projection, maxZoom: 19.5, workers });
// For the console
(window as unknown as { map: GeoMap }).map = map;

function buttons<T extends string>(parent: string, options: readonly T[], initial: T, pick: (o: T) => void): void {
  const row = $(parent);
  for (const option of options) {
    const b = document.createElement('button');
    b.textContent = option;
    b.classList.toggle('on', option === initial);
    b.onclick = () => {
      for (const other of row.children) other.classList.toggle('on', other === b);
      pick(option);
    };
    row.append(b);
  }
}
buttons('projections', ['globe', 'mercator'] as Projection[], projection, (p) => {
  projection = p;
  map.setProjection(p);
});
buttons('basemaps', ['plan-grey', 'plan', 'ortho', 'none'] as Basemap[], 'plan-grey', (b) =>
  map.setBasemap(b, b === 'ortho' ? 0 : 0.4),
);
// Photomesh of the Métropole Européenne de Lille (2020, 5 cm, open data): a heavy textured 3D Tiles dataset
const LILLE_MESH = 'https://webimaging.lillemetropole.fr/externe/maillage/2020_mel_5cm/tileset.json';
let mesh: Tiles3dLayer | undefined;
$('lille').onclick = () => {
  mesh ??= map.addTiles3d({ url: LILLE_MESH, maxScreenSpaceError: 6 });
  map.easeTo({ center: [3.0632, 50.6369], zoom: 16.5, pitch: 60, bearing: 20 }, 3000);
};
$('fly').onclick = () => map.easeTo({ center: IGN, zoom: 15, pitch: 50, bearing: -17 }, 2500);
$('world').onclick = () => map.easeTo({ center: [2.5, 46.6], zoom: 2.2, pitch: 0, bearing: 0 }, 2500);

// Pointer position
const label = $('label');
map.on('hover', ({ point, lonLat }) => {
  label.style.display = lonLat ? 'block' : 'none';
  if (!lonLat) return;
  label.style.left = `${point[0]}px`;
  label.style.top = `${point[1]}px`;
  label.textContent = `${lonLat[0].toFixed(4)}, ${lonLat[1].toFixed(4)}`;
});

// Stats: frames per second over the last second, engine time per frame, tiles per layer
const stats = $('stats');
const frames: number[] = [];
let engineMs = 0;
let worstMs = 0;
map.on('render', ({ ms }) => {
  const now = performance.now();
  frames.push(now);
  while (frames[0] < now - 1000) frames.shift();
  engineMs = engineMs * 0.9 + ms * 0.1;
  worstMs = Math.max(worstMs * 0.995, ms);
});
setInterval(() => {
  // Idle map: no frames, the counter must drop to 0
  while (frames.length && frames[0] < performance.now() - 1000) frames.shift();
  const c = map.getCamera();
  const layers = map
    .stats()
    .map(
      (l) =>
        `layer ${l.id}    ${l.drawn} drawn (${l.standIns} blurry), ${l.loaded} kept, ${l.inFlight} loading` +
        (l.queued ? `, ${l.queued} queued` : '') +
        (l.cooling ? `, ${l.cooling} retrying` : '') +
        (l.missing ? `, ${l.missing} missing` : ''),
    );
  stats.textContent = [
    `zoom       ${c.zoom.toFixed(2)}`,
    `fps        ${frames.length}`,
    `engine     ${engineMs.toFixed(2)} ms/frame, worst ${worstMs.toFixed(1)}`,
    ...layers,
    ...(mesh
      ? [
          ((m) => `3D tiles   ${m.drawn} drawn, ${m.loaded} kept (${m.gpuMb.toFixed(0)} MB), ${m.inFlight} loading`)(
            mesh.stats(),
          ),
        ]
      : []),
  ].join('\n');
}, 250);
