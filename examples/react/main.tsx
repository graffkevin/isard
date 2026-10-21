/**
 * React: the map and what it shows as components (isard/react). Each click on the map adds a stop, a marker with its
 * popup; a line joins the stops (its data updated, not the layer made anew); the panel's controls change the line's
 * style and the basemap through props. Removing a stop unmounts its marker.
 */
import type { Basemap, LonLat } from 'isard';
import { IsardMap, Layer, Marker, Popup } from 'isard/react';
import { useMemo, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { element } from '../shared/page.ts';

interface Stop {
  id: number;
  lonLat: LonLat;
}

const START: Stop[] = [
  { id: 1, lonLat: [2.3376, 48.8606] },
  { id: 2, lonLat: [2.3499, 48.853] },
  { id: 3, lonLat: [2.3644, 48.8589] },
];
const BASEMAPS: Basemap[] = ['ign-plan-grey-vector', 'ign-plan-vector', 'ign-ortho'];

function App() {
  const [stops, setStops] = useState(START);
  const [width, setWidth] = useState(4);
  const [color, setColor] = useState('#0057b8');
  const [basemap, setBasemap] = useState<Basemap>('ign-plan-grey-vector');
  // A new object only when the stops change: the line's data is sent again then only
  const route = useMemo(
    () => ({
      type: 'FeatureCollection' as const,
      features:
        stops.length < 2
          ? []
          : [
              {
                type: 'Feature' as const,
                properties: {},
                geometry: { type: 'LineString' as const, coordinates: stops.map((s) => s.lonLat) },
              },
            ],
    }),
    [stops],
  );

  return (
    <>
      <IsardMap
        id="map"
        basemap={basemap}
        center={[2.3499, 48.8566]}
        zoom={14}
        projection="mercator"
        onClick={({ lonLat }) => {
          if (lonLat) setStops((all) => [...all, { id: Math.max(0, ...all.map((s) => s.id)) + 1, lonLat }]);
        }}
      >
        <Layer type="lines" data={route} color={color} width={width} />
        {stops.map((stop, index) => (
          <Marker
            key={stop.id}
            lonLat={stop.lonLat}
            draggable
            onDragEnd={(lonLat) => setStops((all) => all.map((s) => (s.id === stop.id ? { ...s, lonLat } : s)))}
          >
            <Popup offset={[0, -34]}>
              <b>Stop {index + 1}</b>
              <br />
              {stop.lonLat[0].toFixed(4)}, {stop.lonLat[1].toFixed(4)}
              <br />
              <button type="button" onClick={() => setStops((all) => all.filter((s) => s.id !== stop.id))}>
                Remove
              </button>
            </Popup>
          </Marker>
        ))}
      </IsardMap>
      <div className="panel">
        <h1>React</h1>
        <p>Click the map to add a stop, drag a stop to move it, click it to remove it.</p>
        <p>
          {BASEMAPS.map((b) => (
            <button key={b} type="button" className={b === basemap ? 'on' : ''} onClick={() => setBasemap(b)}>
              {b.replace('ign-', '').replace('-vector', '')}
            </button>
          ))}
        </p>
        <label>
          Width <output>{width}</output> px
          <input type="range" min={1} max={12} value={width} onChange={(e) => setWidth(Number(e.target.value))} />
        </label>
        <label>
          Color <input type="color" value={color} onChange={(e) => setColor(e.target.value)} />
        </label>
      </div>
    </>
  );
}

createRoot(element('root')).render(<App />);
