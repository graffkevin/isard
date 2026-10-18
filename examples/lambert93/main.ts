/**
 * Data in Lambert-93 on a Web Mercator map: the Géoplateforme's PLAN IGN as served in its Lambert-93 tile matrix set
 * (2154_10cm_6_20), reprojected by the map's workers, over OpenFreeMap's basemap (or the same map in its Web Mercator
 * grid, to compare); the arrondissements of Paris from its WFS in EPSG:2154 (the collection names its CRS, the layers
 * reproject it); a GPX file: a boat trip down the Seine and its bridges.
 */
import { GeoMap, geoplateformeWfs, geoplateformeWmts, IGN_ATTRIBUTION, parseGpx, type RasterLayer } from 'isard';
import { element } from '../shared/page.ts';

const map = await GeoMap.create(element('map'), {
  center: [2.317, 48.8595],
  zoom: 13.2,
  projection: 'mercator',
  attributionControl: { customAttribution: IGN_ATTRIBUTION },
});

/** PLAN IGN in each grid of the Géoplateforme: its layer, matrix set and format */
const GRIDS = {
  '2154_10cm_6_20': ['GEOGRAPHICALGRIDSYSTEMS.PLANIGNV2.L93', 'image/jpeg'],
  PM: ['GEOGRAPHICALGRIDSYSTEMS.PLANIGNV2', 'image/png'],
} as const;

/** PLAN IGN in a grid: Lambert-93 (reprojected) or Web Mercator (as the engine draws it) */
const plan = (matrixSet: keyof typeof GRIDS) => {
  const [layer, format] = GRIDS[matrixSet];
  return map.addRaster(geoplateformeWmts(layer, { matrixSet, format, opacity: 0.6 }));
};
let raster: RasterLayer = plan('2154_10cm_6_20');

// The arrondissements in Lambert-93: their `crs` member is enough
const arrondissements = await geoplateformeWfs(
  'ADMINEXPRESS-COG.LATEST:arrondissement_municipal',
  [2.25, 48.83, 2.4, 48.89],
  {
    srsName: 'EPSG:2154',
  },
);
const boundaries = map.addLines({ data: arrondissements, color: '#7a2e8e', width: 2.5, dash: [2, 1.5] });

// A GPX file: the track as a line, the waypoints as discs and labels
const trip = parseGpx(await (await fetch('seine.gpx')).text());
map.addLines({ data: trip, color: '#fff', width: 7, opacity: 0.9 });
map.addLines({ data: trip, color: '#1764c4', width: 3.5 });
map.addCircles({ data: trip, color: '#fff', radius: 5, strokeColor: '#1764c4', strokeWidth: 2.5 });
map.addSymbols({
  data: trip,
  text: (p) => String(p.name ?? ''),
  filter: (p) => p.time !== undefined,
  size: 12,
  color: '#0f3d7a',
  haloColor: '#fff',
  haloWidth: 1.5,
  // Above and below the river in turn, over the basemap's labels of the river
  anchor: (_, i) => (i % 2 ? 'top' : 'bottom'),
  offset: (_, i) => [0, i % 2 ? 0.7 : -0.7],
  overlap: true,
});

for (const id of Object.keys(GRIDS) as (keyof typeof GRIDS)[]) {
  element(id).onclick = () => {
    const opacity = Number(element<HTMLInputElement>('opacity').value);
    raster.remove();
    raster = plan(id);
    raster.setStyle({ opacity });
    raster.moveBefore(boundaries);
    for (const other of Object.keys(GRIDS)) element(other).classList.toggle('on', other === id);
  };
}
element<HTMLInputElement>('opacity').oninput = (e) =>
  raster.setStyle({ opacity: Number((e.target as HTMLInputElement).value) });
