/**
 * Viewshed in React: what can be seen from points. From the forecourt of the Sacré-Cœur on the hill of Montmartre,
 * over the IGN's relief (RGE ALTI) and the BD TOPO buildings raised to their height: the scene is drawn from each
 * observer into a depth cube, and each pixel on screen tinted by whether something stands between it and every
 * observer that reaches it. The observers are a state, each a `<Marker>` with its `<Popup>` and a `<Viewshed>`. A click
 * on the map adds an observer; a click on its pin opens its settings (one popup open at a time), and the pin drags. On
 * a building, the observer stands on its roof.
 */
import { type FeatureCollection, geoplateformeWfs, type LonLat, type Position } from 'isard';
import { IsardMap, Layer } from 'isard/react';
import { useEffect, useMemo, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { element } from '../../shared/page.ts';
import { ObserverPins, ObserversPanel, useObservers } from './observers.tsx';

type Building = { hauteur: number | null };
type Ring = Position[];

const roofHeight = (p: Building) => p.hauteur ?? 6;

const polygonsOf = (g: FeatureCollection<Building>['features'][number]['geometry']): Ring[][] => {
  if (g?.type === 'Polygon') return [g.coordinates];
  if (g?.type === 'MultiPolygon') return g.coordinates;

  return [];
};
const inside = ([x, y]: LonLat, ring: Ring) => {
  let within = false;

  for (let i = 0, j = ring.length - 1; i < ring.length; j = i++) {
    const [xi, yi] = ring[i] as Position;
    const [xj, yj] = ring[j] as Position;

    if (yi > y !== yj > y && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi) within = !within;
  }

  return within;
};
/** The metres of the roof under a point (0 off the buildings), from the buildings' outlines and bounding boxes */
const roofFinder = (buildings: FeatureCollection<Building>) => {
  const footprints = buildings.features.flatMap((f) =>
    polygonsOf(f.geometry).map((rings) => {
      const outer = rings[0] ?? [];
      const lons = outer.map((c) => c[0]);
      const lats = outer.map((c) => c[1]);
      const box = [Math.min(...lons), Math.min(...lats), Math.max(...lons), Math.max(...lats)];

      return { rings, box, height: roofHeight(f.properties) };
    }),
  );

  return (at: LonLat) => {
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
};

const App = () => {
  const [buildings, setBuildings] = useState<FeatureCollection<Building>>();

  useEffect(() => {
    geoplateformeWfs<Building>('BDTOPO_V3:batiment', [2.332, 48.877, 2.356, 48.893]).then(setBuildings, console.error);
  }, []);

  const roofAt = useMemo(() => buildings && roofFinder(buildings), [buildings]);
  const observers = useObservers([2.34305, 48.88615]);

  return (
    <>
      <IsardMap
        id="map"
        center={[2.3435, 48.8848]}
        zoom={15.6}
        pitch={55}
        bearing={160}
        basemap="ign-plan-grey-vector"
        terrain={{ model: 'ign' }}
        projection="mercator"
      >
        {buildings && roofAt && (
          <>
            <Layer type="extrusions" data={buildings} height={roofHeight} color="#e4e1d8" />
            <ObserverPins
              observers={observers}
              standOn={(at) => {
                const metres = roofAt(at);

                return metres > 0 ? { metres, what: 'a roof' } : undefined;
              }}
            />
          </>
        )}
      </IsardMap>
      <ObserversPanel title="Viewshed" observers={observers} />
    </>
  );
};

createRoot(element('root')).render(<App />);
