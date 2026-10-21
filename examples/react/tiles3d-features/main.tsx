/**
 * 3D Tiles features in React: a city of blocks from the 3D Tiles samples (Cesium's, b3dm with batch tables) picked
 * one by one, each with its properties (`pickFeature` on a 3D Tiles layer: the batch table of 3D Tiles 1.0, the
 * property table of `EXT_structural_metadata` in 1.1) in a `<Popup>`; the layer tinted by a color (its `color` prop,
 * sent by `setStyle`).
 */
import type { GeoMap, LonLat, Tiles3dLayer } from 'isard';
import { IsardMap, Layer, Popup } from 'isard/react';
import { useRef, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { element } from '../../shared/page.ts';

interface Picked {
  lonLat: LonLat;
  properties: Record<string, unknown>;
}

const format = (value: unknown) => (typeof value === 'number' ? String(Math.round(value * 1e6) / 1e6) : String(value));

const App = () => {
  const map = useRef<GeoMap>(undefined);
  const city = useRef<Tiles3dLayer>(undefined);
  const [color, setColor] = useState('#9ec5fe');
  const [picked, setPicked] = useState<Picked>();

  return (
    <>
      <IsardMap
        ref={map}
        id="map"
        center={[-75.6123, 40.0425]}
        zoom={17.6}
        pitch={55}
        bearing={30}
        basemap="osm-positron"
        projection="mercator"
        tiles3d
        onClick={({ point, lonLat }) => {
          const hit = city.current && map.current?.pickFeature(point, { layers: [city.current] });

          setPicked(hit?.feature && lonLat ? { lonLat, properties: hit.feature.properties ?? {} } : undefined);
        }}
      >
        <Layer
          ref={city}
          type="tiles3d"
          url="https://raw.githubusercontent.com/CesiumGS/3d-tiles-samples/main/1.0/TilesetWithRequestVolume/city/tileset.json"
          color={color}
        />
        {/* The map's click decides: another building moves it, nothing closes it */}
        {picked && (
          <Popup lonLat={picked.lonLat} maxWidth="260px" closeOnClick={false} onClose={() => setPicked(undefined)}>
            <table>
              <tbody>
                {Object.entries(picked.properties).map(([name, value]) => (
                  <tr key={name}>
                    <td>{name}</td>
                    <td>{format(value)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </Popup>
        )}
      </IsardMap>
      <div className="panel">
        <h1>3D Tiles features</h1>
        <p>Click a building: its properties, from the tile's batch table. The color tints the whole layer.</p>
        <p>
          <label>
            Color <input type="color" value={color} onChange={(e) => setColor(e.target.value)} />
          </label>
        </p>
      </div>
    </>
  );
};

createRoot(element('root')).render(<App />);
