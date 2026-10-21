/** A map in React: the default basemap (OpenFreeMap's Positron, OpenStreetMap data), the navigation control, a marker
 * with its popup */
import { type GeoMap, type LonLat, NavigationControl } from 'isard';
import { Control, IsardMap, Marker, Popup } from 'isard/react';
import { useMemo, useRef, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { element } from '../../shared/page.ts';

const VINCENNES: LonLat = [2.4358, 48.8427];

const App = () => {
  const map = useRef<GeoMap>(undefined);
  const [lonLat, setLonLat] = useState(VINCENNES);
  const navigation = useMemo(() => new NavigationControl({ visualizePitch: true }), []);

  return (
    <>
      <IsardMap
        ref={map}
        id="map"
        center={VINCENNES}
        zoom={15}
        projection="mercator"
        // A click elsewhere moves the marker there, a short flight follows it
        onClick={({ lonLat }) => {
          if (!lonLat) return;
          setLonLat(lonLat);
          map.current?.easeTo({ center: lonLat }, 500);
        }}
      >
        <Control control={navigation} position="top-right" />
        <Marker lonLat={lonLat} draggable onDragEnd={setLonLat}>
          <Popup>
            <b>Château de Vincennes</b>
            <br />
            Drag the marker, or click the map
          </Popup>
        </Marker>
      </IsardMap>
      <div className="panel">
        <h1>A map</h1>
        <p>
          The default basemap (OpenFreeMap, OpenStreetMap data), controls, a marker with its popup. Click the map to
          move the marker.
        </p>
      </div>
    </>
  );
};

createRoot(element('root')).render(<App />);
