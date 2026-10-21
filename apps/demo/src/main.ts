/**
 * Demo of the engine: OpenFreeMap and Géoplateforme basemaps on the globe or in Web Mercator, the Lille photomesh in
 * 3D Tiles, the worldwide relief, and the loading counters of every layer. URL parameters: ?projection=mercator, ?workers=0 (3D Tiles
 * decoded on the main thread, to compare), ?terrain=1 (relief on from the start).
 */
import {
  type Basemap,
  GeoMap,
  type LonLat,
  Marker,
  NavigationControl,
  Popup,
  type Projection,
  type Tiles3dLayer,
} from 'isard';

declare global {
  interface Window {
    /** The demo map, for the console */
    map: GeoMap;
  }
}

const PARIS: LonLat = [2.3376, 48.8606];
const WORLD = { center: [2.5, 46.6] as LonLat, zoom: 2.2, pitch: 0, bearing: 0 };
/** The worldwide relief, raised by the French geoid: the photomesh's ellipsoidal heights sit on it in France */
const RELIEF = { geoid: true };
/** Photomesh of the Métropole Européenne de Lille (2020, 5 cm, open data): a heavy textured 3D Tiles dataset */
const LILLE_MESH = 'https://webimaging.lillemetropole.fr/externe/maillage/2020_mel_5cm/tileset.json';

const params = new URLSearchParams(location.search);
const projection: Projection = params.get('projection') === 'mercator' ? 'mercator' : 'globe';

const element = <T extends HTMLElement = HTMLElement>(id: string): T => {
  const found = document.getElementById(id);
  if (!found) throw new Error(`#${id} missing from the page`);
  return found as T;
};

const map = await GeoMap.create(element('map'), {
  ...WORLD,
  projection,
  maxZoom: 19.5,
  terrain: params.get('terrain') === '1' && RELIEF,
  ...(params.get('workers') === '0' ? { workers: false } : {}),
});
window.map = map;
map.addControl(new NavigationControl({ visualizePitch: true }));

// A draggable marker at the Louvre; a click opens its popup, which follows it when dragged
const popup = new Popup({ offset: [0, -34] });
const pin = new Marker({ draggable: true }).setLonLat(PARIS).setPopup(popup).addTo(map);
const where = () => {
  const [lon, lat] = pin.getLonLat();
  popup.setHTML(
    `<strong>Pin</strong><br>${lon.toFixed(5)}, ${lat.toFixed(5)}<br><small>Drag the pin to move it</small>`,
  );
};
where();
pin.on('drag', where);

/** A row of toggle buttons */
const buttons = <T extends string>(
  parent: string,
  options: readonly T[],
  initial: T,
  pick: (option: T) => void,
): void => {
  const row = element(parent);
  for (const option of options) {
    const button = document.createElement('button');
    button.type = 'button';
    button.textContent = option;
    button.classList.toggle('on', option === initial);
    button.onclick = () => {
      for (const other of row.children) other.classList.toggle('on', other === button);
      pick(option);
    };
    row.append(button);
  }
};

const projections: readonly Projection[] = ['globe', 'mercator'];
const basemaps: readonly Basemap[] = [
  'osm-positron',
  'osm-liberty',
  'ign-plan',
  'ign-plan-grey-vector',
  'ign-ortho',
  'none',
];
buttons('projections', projections, projection, (p) => map.setProjection(p));
buttons('basemaps', basemaps, 'osm-positron', (b) => map.setBasemap(b));
element('fly').onclick = () => map.easeTo({ center: PARIS, zoom: 15, pitch: 50, bearing: -17 }, 2500);
element('world').onclick = () => map.easeTo(WORLD, 2500);

let mesh: Tiles3dLayer | undefined;
element('lille').onclick = () => {
  mesh ??= map.addTiles3d({ url: LILLE_MESH, maxScreenSpaceError: 6 });
  map.easeTo({ center: [3.0632, 50.6369], zoom: 16.5, pitch: 60, bearing: 20 }, 3000);
};

// Relief: the worldwide Terrain Tiles under the basemap
const relief = element('relief');
let terrain = params.get('terrain') === '1';
relief.classList.toggle('on', terrain);
relief.onclick = () => {
  terrain = !terrain;
  relief.classList.toggle('on', terrain);
  map.setTerrain(terrain && RELIEF);
};
element('montblanc').onclick = () => {
  if (!terrain) relief.click();
  map.setBasemap('ign-ortho');
  for (const button of element('basemaps').children) button.classList.toggle('on', button.textContent === 'ign-ortho');
  map.easeTo({ center: [6.87, 45.9], zoom: 12.5, pitch: 72, bearing: 160 }, 3000);
};

// Pointer position
const label = element('label');
map.on('hover', ({ point, lonLat }) => {
  label.style.display = lonLat ? 'block' : 'none';
  if (!lonLat) return;
  label.style.left = `${point[0]}px`;
  label.style.top = `${point[1]}px`;
  label.textContent = `${lonLat[0].toFixed(4)}, ${lonLat[1].toFixed(4)}`;
});
map.on('leave', () => {
  label.style.display = 'none';
});

// Counters: frames over the last second, engine time per frame, tiles per layer
const frames: number[] = [];
let engineMs = 0;
let worstMs = 0;
map.on('render', ({ ms }) => {
  frames.push(performance.now());
  engineMs = engineMs * 0.9 + ms * 0.1;
  worstMs = Math.max(worstMs * 0.995, ms);
});

const stats = element('stats');
setInterval(() => {
  // An idle map draws nothing: the counter must drop to 0
  const now = performance.now();
  while (frames.length > 0 && (frames[0] ?? now) < now - 1000) frames.shift();
  const camera = map.getCamera();
  const raster = map.stats().map((l) => {
    const extra = [
      l.queued ? `${l.queued} queued` : '',
      l.cooling ? `${l.cooling} retrying` : '',
      l.missing ? `${l.missing} missing` : '',
    ].filter(Boolean);
    return `layer ${l.id}    ${l.drawn} drawn (${l.standIns} blurry), ${l.loaded} kept, ${l.inFlight} loading${extra.length ? `, ${extra.join(', ')}` : ''}`;
  });
  const m = mesh?.stats();
  stats.textContent = [
    `zoom       ${camera.zoom.toFixed(2)}`,
    `fps        ${frames.length}`,
    `engine     ${engineMs.toFixed(2)} ms/frame, worst ${worstMs.toFixed(1)}`,
    ...raster,
    ...(m ? [`3D tiles   ${m.drawn} drawn, ${m.loaded} kept (${m.gpuMb.toFixed(0)} MB), ${m.inFlight} loading`] : []),
  ].join('\n');
}, 250);
