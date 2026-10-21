/**
 * Panoramas in React: the oriented images of a vehicle's cameras (iTowns' immersive sample of Paris, rue Servandoni)
 * projected onto the scene around the panorama shown, the BD TOPO buildings raised to take them. From above, the
 * panorama colors its street; the page starts walking in, the eye at the cameras: the drag turns it, the wheel goes to
 * the nearest panorama ahead (or behind), a click on the street to the one nearest to the point clicked, Z and S to the
 * next and the previous ones. The panoramas have no component: a child of the map adds them (`useMap`).
 */
import { type BdTopoBuilding, bdtopoBuildings, type ExtrusionOptions, type Panoramas } from 'isard';
import { IsardMap, Layer, useMap } from 'isard/react';
import { useEffect, useReducer, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { element } from '../../shared/page.ts';

const SAMPLE = 'https://raw.githubusercontent.com/iTowns/iTowns2-sample-data/master/immersive/exampleParis1';

/** Adds the panoramas to the map around and walks in, at the first one; `onKey` after a key changed the one shown */
const Street = ({ onReady, onKey }: { onReady: (panoramas: Panoramas) => void; onKey: () => void }) => {
  const map = useMap();

  useEffect(() => {
    const container = map?.getContainer();

    container?.addEventListener('keyup', onKey);

    return () => container?.removeEventListener('keyup', onKey);
  }, [map, onKey]);
  useEffect(() => {
    if (!map) return;

    let cancelled = false;

    map
      .addPanoramas({
        url: 'https://www.itowns-project.org/itowns-sample-data-small/images/140616/Paris-140616_0740-{cameraId}-00001_0000{panoId}.jpg',
        orientationsUrl: `${SAMPLE}/panoramicsMetaDataParis.geojson`,
        calibrationUrl: `${SAMPLE}/cameraCalibration.json`,
        backgroundDistance: 1200,
      })
      .then(async (panoramas) => {
        // Straight in the street, at the first panorama
        await panoramas.enter({ index: 0 });
        if (!cancelled) onReady(panoramas);
      })
      .catch(console.error);

    return () => {
      cancelled = true;
    };
  }, [map, onReady]);

  return null;
};

const App = () => {
  const [buildings, setBuildings] = useState<ExtrusionOptions<BdTopoBuilding>>();

  useEffect(() => {
    bdtopoBuildings([2.331, 48.847, 2.338, 48.853], { defaultHeight: 10 }).then(setBuildings, console.error);
  }, []);

  const [panoramas, setPanoramas] = useState<Panoramas>();
  const [walking, setWalking] = useState(true);
  // The panorama shown changes in the map (a click, a key, the wheel): read again after them
  const [, refresh] = useReducer((n: number) => n + 1, 0);
  const pano = panoramas?.current;

  const toggle = async () => {
    if (!panoramas) return;
    setWalking(!walking);
    if (walking) {
      panoramas.exit();
      await panoramas.show(0);
    } else await panoramas.enter({ index: panoramas.current?.index ?? 0 });
    refresh();
  };

  return (
    <>
      <IsardMap
        id="map"
        center={[2.3346, 48.8505]}
        zoom={18.6}
        pitch={60}
        bearing={176}
        preset="ign-3d"
        onClick={() => setTimeout(refresh, 0)}
      >
        {buildings && (
          <>
            <Layer {...buildings} type="extrusions" />
            <Street onReady={setPanoramas} onKey={refresh} />
          </>
        )}
      </IsardMap>
      <div className="panel">
        <h1>Panoramas</h1>
        <p>Street-level images of Paris (iTowns' sample data) projected onto the BD TOPO buildings.</p>
        <p>
          Drag to look around, the wheel to walk along the street (or click it), Z and S for the next and the previous
          panoramas.
        </p>
        <p>
          <button type="button" onClick={toggle}>
            {walking ? 'Leave' : 'Walk in'}
          </button>{' '}
          <span>{pano && panoramas ? `panorama ${pano.index + 1} of ${panoramas.list.length}` : ''}</span>
        </p>
      </div>
    </>
  );
};

createRoot(element('root')).render(<App />);
