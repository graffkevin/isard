/**
 * Vector basemaps in React: OpenFreeMap's styles drawn from its vector tiles (OpenStreetMap data, the OpenMapTiles
 * schema), and a subset of a style on its own raster layer, restyled: the rivers and water areas in a strong blue, over
 * the basemap (they could go over a flood layer of the application).
 */
import type { RasterLayer } from 'isard';
import { IsardMap, useMap } from 'isard/react';
import { useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { element } from '../../shared/page.ts';

const STYLES = ['positron', 'liberty', 'bright'] as const;
type Style = (typeof STYLES)[number];

/** The rivers of the basemap's data, drawn by their own layers (vector tiles have no component) */
const Rivers = ({ shown }: { shown: boolean }) => {
  const map = useMap();
  const [rivers, setRivers] = useState<RasterLayer>();

  // The rivers' layer drawn from the basemap's vector tiles once the map is there; its tiles freed when unmounted
  useEffect(() => {
    if (!map) return;

    const tiles = map.vectorTiles();

    // The water areas and the rivers, in the style's source and its source layers
    setRivers(
      tiles.raster({
        layers: () => false,
        extraLayers: [
          {
            id: 'water',
            type: 'fill',
            'source-layer': 'water',
            paint: { 'fill-color': '#2f7fc4', 'fill-opacity': 0.55 },
          },
          {
            id: 'rivers',
            type: 'line',
            'source-layer': 'waterway',
            layout: { 'line-cap': 'round', 'line-join': 'round' },
            paint: {
              'line-color': '#2f7fc4',
              'line-width': {
                stops: [
                  [10, 1],
                  [16, 5],
                ],
              },
            },
          },
        ],
      }),
    );

    return () => {
      tiles.destroy();
    };
  }, [map]);
  // The rivers shown or hidden with the checkbox
  useEffect(() => {
    rivers?.setStyle({ opacity: shown ? 1 : 0 });
  }, [rivers, shown]);

  return null;
};

const App = () => {
  const [style, setStyle] = useState<Style>('positron');
  const [rivers, setRivers] = useState(true);

  return (
    <>
      <IsardMap id="map" center={[1.909, 47.902]} zoom={13} basemap={`osm-${style}`} projection="mercator">
        <Rivers shown={rivers} />
      </IsardMap>
      <div className="panel">
        <h1>Vector basemaps</h1>
        <p>
          OpenFreeMap's styles drawn from its vector tiles (OpenStreetMap data), and a subset of a style on its own
          layer: the rivers, restyled.
        </p>
        <p>
          {STYLES.map((id) => (
            <button key={id} type="button" className={id === style ? 'on' : ''} onClick={() => setStyle(id)}>
              {id}
            </button>
          ))}{' '}
          <label>
            <input type="checkbox" checked={rivers} onChange={(e) => setRivers(e.target.checked)} /> rivers
          </label>
        </p>
      </div>
    </>
  );
};

createRoot(element('root')).render(<App />);
