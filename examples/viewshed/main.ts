/**
 * Viewshed: what can be seen from points. From the forecourt of the Sacré-Cœur on the hill of Montmartre, over the
 * IGN's relief (RGE ALTI) and the BD TOPO buildings raised to their height: the scene is drawn from each observer
 * into a depth cube, and each pixel on screen tinted by whether something stands between it and every observer that
 * reaches it. A click on the map adds an observer; a click on its pin opens its settings (one popup open at a time),
 * and the pin drags. On a building, the observer stands on its roof.
 */
import { GeoMap, geoplateformeWfs, type LonLat } from 'isard';
import { observers } from '../shared/observers.ts';
import { element } from '../shared/page.ts';

type Building = { hauteur: number | null };

const map = await GeoMap.create(element('map'), {
  center: [2.3435, 48.8848],
  zoom: 15.6,
  pitch: 55,
  bearing: 160,
  basemap: 'ign-plan-grey-vector',
  terrain: { model: 'ign' },
  projection: 'mercator',
});

const buildings = await geoplateformeWfs<Building>('BDTOPO_V3:batiment', [2.332, 48.877, 2.356, 48.893]);
const roofHeight = (p: Building) => p.hauteur ?? 6;
map.addExtrusions({ data: buildings, height: roofHeight, color: '#e4e1d8' });

// The buildings' outlines and bounding boxes, to find the roof under a pin
type Ring = LonLat[];
const footprints = buildings.features.flatMap((f) => {
  const g = f.geometry;
  let polygons: Ring[][] = [];
  if (g?.type === 'Polygon') polygons = [g.coordinates as Ring[]];
  else if (g?.type === 'MultiPolygon') polygons = g.coordinates as Ring[][];
  return polygons.map((rings) => {
    const outer = rings[0] ?? [];
    const lons = outer.map((c) => c[0]);
    const lats = outer.map((c) => c[1]);
    const box = [Math.min(...lons), Math.min(...lats), Math.max(...lons), Math.max(...lats)];
    return { rings, box, height: roofHeight(f.properties) };
  });
});
const inside = ([x, y]: LonLat, ring: Ring) => {
  let within = false;
  for (let i = 0, j = ring.length - 1; i < ring.length; j = i++) {
    const [xi, yi] = ring[i] as LonLat;
    const [xj, yj] = ring[j] as LonLat;
    if (yi > y !== yj > y && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi) within = !within;
  }
  return within;
};
/** Metres of the roof under a point, 0 off the buildings */
const roofAt = (at: LonLat) => {
  const [x, y] = at;
  const under = footprints.find(
    ({ rings, box }) =>
      x >= (box[0] ?? 0) &&
      y >= (box[1] ?? 0) &&
      x <= (box[2] ?? 0) &&
      y <= (box[3] ?? 0) &&
      inside(at, rings[0] ?? []) &&
      !rings.slice(1).some((hole) => inside(at, hole)),
  );
  return under?.height ?? 0;
};

observers(map, {
  standOn: (at) => {
    const metres = roofAt(at);
    return metres > 0 ? { metres, what: 'a roof' } : undefined;
  },
})([2.34305, 48.88615]);
