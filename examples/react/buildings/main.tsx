/**
 * Buildings in React: the IGN's BD TOPO buildings (Géoplateforme WFS), over PLAN.IGN, raised to their height and
 * colored by it through a ramp, with depth; the building under the cursor described. A tilted view, the relief off.
 */
import {
  type ExtrusionLayer,
  type FeatureCollection,
  geoplateformeWfs,
  type Layer as LayerHandle,
  type Ramp,
} from 'isard';
import { IsardMap, Layer, useMap } from 'isard/react';
import { useEffect, useMemo, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { element } from '../../shared/page.ts';

type Building = { hauteur: number | null; usage_1: string | null; nombre_de_logements: number | null };

// Outside the component: the same function and ramp at each render, the layer not updated for them
const height = (p: Building) => p.hauteur ?? 6;
const RAMP: Ramp = [
  [0, '#fde0c5'],
  [12, '#f59e72'],
  [25, '#d0587e'],
  [45, '#6c2167'],
];

const describe = (p: Building | undefined) =>
  p
    ? `${p.usage_1 ?? 'Building'}, ${p.hauteur ?? '?'} m${p.nombre_de_logements ? `, ${p.nombre_de_logements} homes` : ''}`
    : ' ';

/** The layers 'hoverfeature' picks among (`map.hoverPicking`) */
const HoverPicking = ({ layers }: { layers: LayerHandle[] }) => {
  const map = useMap();
  useEffect(() => {
    if (map) map.hoverPicking = { layers };
  }, [map, layers]);
  return null;
};

const App = () => {
  const [buildings, setBuildings] = useState<FeatureCollection<Building>>();
  const [layer, setLayer] = useState<LayerHandle>();
  const [info, setInfo] = useState(' ');
  const picked = useMemo(() => (layer ? [layer] : []), [layer]);
  useEffect(() => {
    geoplateformeWfs<Building>('BDTOPO_V3:batiment', [2.414, 48.84, 2.432, 48.851]).then(setBuildings);
  }, []);

  return (
    <>
      <IsardMap
        id="map"
        center={[2.4225, 48.8452]}
        zoom={16.4}
        pitch={60}
        bearing={-25}
        basemap="ign-plan-grey-vector"
        projection="mercator"
        onHoverFeature={({ hit }) => setInfo(describe(hit?.feature?.properties as Building | undefined))}
      >
        {buildings && (
          <Layer
            type="extrusions"
            data={buildings}
            height={height}
            color="#d9d6cc"
            ramp={RAMP}
            onLayer={(created) => {
              // The values of the ramp: a method of the handle, not a prop
              (created as ExtrusionLayer<Building>).setValues(
                Float32Array.from(buildings.features, (f) => f.properties.hauteur ?? Number.NaN),
              );
              setLayer(created);
            }}
          />
        )}
        <HoverPicking layers={picked} />
      </IsardMap>
      <div className="panel">
        <h1>Buildings</h1>
        <p>BD TOPO buildings raised to their height and colored by it. Hover a building.</p>
        <p id="info">{info}</p>
      </div>
    </>
  );
};

createRoot(element('root')).render(<App />);
