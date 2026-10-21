/**
 * Point clouds: the IGN's LiDAR HD tile of the Île de la Cité (1 km², 15.5 million points, Licence Ouverte), a COPC
 * file read by HTTP range requests on the Géoplateforme, its nodes decoded in the workers. Its points stand on the
 * IGN relief: their NGF-IGN69 altitudes are raised by the RAF20 geoid height, as the relief's are. Colored by class,
 * altitude or intensity, with Eye-Dome Lighting; the point under the cursor is picked. `?budget=` sets the point
 * budget (the visual tests keep it small).
 */
import { GeoMap, type PointColor } from 'isard';
import { element } from '../shared/page.ts';

const budget = Number(new URLSearchParams(location.search).get('budget')) || 2_000_000;

const map = await GeoMap.create(element('map'), {
  center: [2.3497, 48.8531],
  zoom: 17.2,
  pitch: 60,
  bearing: -60,
  basemap: 'ign-ortho',
  terrain: 'ign',
  projection: 'mercator',
  // The point cloud at once: the engine that draws it from the start
  tiles3d: true,
});

// A tile of LiDAR HD: its URL from the Géoplateforme's WFS (IGNF_LIDAR-HD_METADONNEE:metadata, url_npl)
const cloud = map.addPointCloud({
  url: 'https://data.geopf.fr/telechargement/download/LiDARHD-NUALID/NUALHD_1-0__LAZ_LAMB93_KE_2025-06-06/LHD_FXX_0652_6862_PTS_LAMB93_IGN69.copc.laz',
  colorBy: 'classification',
  edl: true,
  pointBudget: budget,
});

const names: Record<number, string> = {
  1: 'unclassified',
  2: 'ground',
  3: 'low vegetation',
  4: 'medium vegetation',
  5: 'high vegetation',
  6: 'building',
  9: 'water',
  17: 'bridge',
  64: 'permanent structure',
  66: 'virtual point',
  67: 'miscellaneous building',
};

const modes: PointColor[] = ['classification', 'elevation', 'intensity'];
modes.forEach((mode) => {
  element(mode).addEventListener('click', () => {
    cloud.setStyle({ colorBy: mode });
    modes.forEach((m) => {
      element(m).classList.toggle('on', m === mode);
    });
  });
});
element<HTMLInputElement>('edl').addEventListener('change', (e) => {
  cloud.setStyle({ edl: (e.target as HTMLInputElement).checked });
});

map.on('render', () => {
  const s = cloud.stats();

  if (s && 'points' in s) {
    element('stats').textContent = `${(s.points / 1e6).toFixed(2)} M points in ${s.nodes} nodes, ${s.inFlight} loading`;
  }
});

map.hoverPicking = { layers: [cloud], radius: 3 };
map.on('hoverfeature', ({ hit }) => {
  const p = hit?.feature?.properties as { classification: number; altitude: number } | undefined;

  element('info').textContent = p
    ? `${names[p.classification] ?? `class ${p.classification}`}, ${p.altitude.toFixed(1)} m (IGN69)`
    : ' ';
});
