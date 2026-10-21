/**
 * Point clouds in React: the IGN's LiDAR HD tile of the Île de la Cité (1 km², 15.5 million points, Licence Ouverte),
 * a COPC file read by HTTP range requests on the Géoplateforme, its nodes decoded in the workers. Its points stand on
 * the IGN relief: their NGF-IGN69 altitudes are raised by the RAF20 geoid height, as the relief's are. Colored by
 * class, altitude or intensity, with Eye-Dome Lighting (props of the `<Layer>`, sent by `setStyle`); the point under
 * the cursor is picked. `?budget=` sets the point budget (the visual tests keep it small).
 */
import type { PointCloudLayer, PointColor } from 'isard';
import { IsardMap, Layer } from 'isard/react';
import { Fragment, useMemo, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { element } from '../../shared/page.ts';

const budget = Number(new URLSearchParams(location.search).get('budget')) || 2_000_000;

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

const MODES: { mode: PointColor; label: string }[] = [
  { mode: 'classification', label: 'classes' },
  { mode: 'elevation', label: 'altitude' },
  { mode: 'intensity', label: 'intensity' },
];

const App = () => {
  const [cloud, setCloud] = useState<PointCloudLayer>();
  const [colorBy, setColorBy] = useState<PointColor>('classification');
  const [edl, setEdl] = useState(true);
  const [stats, setStats] = useState('…');
  const [info, setInfo] = useState(' ');
  // The points picked under the cursor ('hoverfeature'), within 3 CSS pixels
  const hoverPicking = useMemo(() => (cloud ? { layers: [cloud], radius: 3 } : {}), [cloud]);

  return (
    <>
      <IsardMap
        id="map"
        center={[2.3497, 48.8531]}
        zoom={17.2}
        pitch={60}
        bearing={-60}
        basemap="ign-ortho"
        terrain="ign"
        projection="mercator"
        // The point cloud at once: the engine that draws it from the start
        tiles3d
        hoverPicking={hoverPicking}
        onRender={() => {
          const s = cloud?.stats();

          if (s && 'points' in s) {
            setStats(`${(s.points / 1e6).toFixed(2)} M points in ${s.nodes} nodes, ${s.inFlight} loading`);
          }
        }}
        onHoverFeature={({ hit }) => {
          const p = hit?.feature?.properties as { classification: number; altitude: number } | undefined;

          setInfo(
            p ? `${names[p.classification] ?? `class ${p.classification}`}, ${p.altitude.toFixed(1)} m (IGN69)` : ' ',
          );
        }}
      >
        {/* A tile of LiDAR HD: its URL from the Géoplateforme's WFS (IGNF_LIDAR-HD_METADONNEE:metadata, url_npl) */}
        <Layer
          type="pointcloud"
          url="https://data.geopf.fr/telechargement/download/LiDARHD-NUALID/NUALHD_1-0__LAZ_LAMB93_KE_2025-06-06/LHD_FXX_0652_6862_PTS_LAMB93_IGN69.copc.laz"
          colorBy={colorBy}
          edl={edl}
          pointBudget={budget}
          onLayer={setCloud}
        />
      </IsardMap>
      <div className="panel">
        <h1>Point clouds</h1>
        <p>
          Notre-Dame de Paris and the Île de la Cité in the IGN's LiDAR HD (Licence Ouverte), read from its COPC file by
          range requests, over the IGN relief. Hover a point.
        </p>
        <p>
          {MODES.map(({ mode, label }) => (
            <Fragment key={mode}>
              <button type="button" className={mode === colorBy ? 'on' : ''} onClick={() => setColorBy(mode)}>
                {label}
              </button>{' '}
            </Fragment>
          ))}
          <label>
            <input type="checkbox" checked={edl} onChange={(e) => setEdl(e.target.checked)} /> EDL
          </label>
        </p>
        <p>
          <output>{stats}</output>
        </p>
        <p>{info}</p>
      </div>
    </>
  );
};

createRoot(element('root')).render(<App />);
