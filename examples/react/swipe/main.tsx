/**
 * Swipe in React: two states of a place on either side of a line. The IGN orthophotos of 1950-1965 over today's
 * (Saint-Denis and the Stade de France, built since), the old ones shown left of the line only (`<Swipe>`); the line
 * drags, its position a state.
 */
import { type GeoMap, geoplateformeWmts } from 'isard';
import { IsardMap, Layer, Swipe } from 'isard/react';
import { useRef, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { element } from '../../shared/page.ts';

const OLD = geoplateformeWmts('ORTHOIMAGERY.ORTHOPHOTOS.1950-1965', { maxZoom: 18 });

const App = () => {
  const map = useRef<GeoMap>(undefined);
  // CSS pixels from the left of the map, once it is there
  const [position, setPosition] = useState<number>();
  const place = (x: number) => setPosition(Math.max(0, Math.min(map.current?.getContainer().clientWidth ?? 0, x)));

  return (
    <>
      <IsardMap
        ref={map}
        id="map"
        center={[2.3601, 48.9245]}
        zoom={15.5}
        basemap="ign-ortho"
        projection="mercator"
        onLoad={(created) => setPosition(created.getContainer().clientWidth / 2)}
        onResize={({ width }) => setPosition((x) => Math.min(width, x ?? width / 2))}
      >
        {position !== undefined && (
          <Swipe position={position}>
            <Layer {...OLD} type="raster" />
          </Swipe>
        )}
      </IsardMap>
      {position !== undefined && (
        <div
          id="line"
          style={{ left: `${position}px` }}
          onPointerDown={(e) => {
            e.currentTarget.setPointerCapture(e.pointerId);
            e.preventDefault();
          }}
          onPointerMove={(e) => {
            if (!e.currentTarget.hasPointerCapture(e.pointerId)) return;
            place(e.clientX - (map.current?.getContainer().getBoundingClientRect().left ?? 0));
          }}
        />
      )}
      <div className="panel">
        <h1>Swipe</h1>
        <p>Left of the line, the orthophotos of 1950-1965; right of it, today's. Drag the line.</p>
      </div>
    </>
  );
};

createRoot(element('root')).render(<App />);
