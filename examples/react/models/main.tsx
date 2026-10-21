/**
 * glTF models in React: a wind turbine (a small GLB next to the TypeScript page) drawn at a row of positions on the
 * IGN relief, one instanced draw, each instance standing on the ground and facing the wind; Cesium's milk truck
 * (Khronos glTF sample, textured) from GitHub. Models load with the engine that draws the 3D Tiles; the instance
 * under the cursor is picked.
 */
import type { ModelInstance, ModelLayer } from 'isard';
import { IsardMap, Layer } from 'isard/react';
import { useMemo, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { element } from '../../shared/page.ts';

const TERRAIN = { model: 'ign', exaggeration: 1.5, maxZoom: 14 } as const;
const TRUCK =
  'https://raw.githubusercontent.com/KhronosGroup/glTF-Sample-Assets/main/Models/CesiumMilkTruck/glTF-Binary/CesiumMilkTruck.glb';

// A row of turbines across the plateau, 250 m apart, the rotors facing the south-west wind
const TURBINES: ModelInstance[] = Array.from({ length: 7 }, (_, i) => ({
  lonLat: [2.7955 + i * 0.0025, 44.183 + i * 0.0013],
  heading: 225,
  scale: 0.9 + 0.05 * (i % 3),
}));
const TRUCKS: ModelInstance[] = [0, 1, 2].map((i) => ({
  lonLat: [2.7992 + i * 0.0003, 44.1846 + i * 0.00015],
  heading: 60,
  scale: 3,
}));

const App = () => {
  const [farm, setFarm] = useState<ModelLayer>();
  const [trucks, setTrucks] = useState<ModelLayer>();
  const [info, setInfo] = useState(' ');
  // The models 'hoverfeature' picks among, within 4 CSS pixels
  const hoverPicking = useMemo(
    () => ({ layers: [farm, trucks].filter((l) => l !== undefined), radius: 4 }),
    [farm, trucks],
  );

  return (
    <>
      <IsardMap
        id="map"
        center={[2.8, 44.1852]}
        zoom={15.7}
        pitch={62}
        bearing={30}
        basemap="osm-liberty"
        terrain={TERRAIN}
        projection="mercator"
        hoverPicking={hoverPicking}
        onHoverFeature={({ hit }) =>
          setInfo(hit ? `${hit.layer === farm ? 'Turbine' : 'Truck'} ${hit.index + 1}` : ' ')
        }
      >
        {/* The GLB of the TypeScript example, its URL relative to this page */}
        <Layer type="models" url="../../models/wind-turbine.glb" instances={TURBINES} onLayer={setFarm} />
        <Layer type="models" url={TRUCK} instances={TRUCKS} onLayer={setTrucks} />
      </IsardMap>
      <div className="panel">
        <h1>glTF models</h1>
        <p>
          Wind turbines on the relief, each one an instance of the same model, and Cesium's milk truck (textured, 3× its
          size) on the plateau. Hover a model.
        </p>
        <p id="info">{info}</p>
      </div>
    </>
  );
};

createRoot(element('root')).render(<App />);
