/**
 * The Moon in React: a map of another body. The globe takes the Moon's radius (no atmosphere, no Earth basemap), the
 * Lunar Reconnaissance Orbiter's wide-angle mosaic comes from NASA's Moon Trek in longitudes and latitudes, reprojected
 * by the workers into Web Mercator tiles; its relief is LOLA's terrain model, from Trek too (`terrain` on the Moon).
 * The Apollo landing sites as circles and labels; pitched views over the craters Tycho and Copernicus (a link opens
 * one: #tycho). A click flies to the point.
 */
import type { Camera, Feature, GeoMap } from 'isard';
import { IsardMap, Layer } from 'isard/react';
import { Fragment, useRef } from 'react';
import { createRoot } from 'react-dom/client';
import { element } from '../../shared/page.ts';

const globe: Camera = { center: [5, 0], zoom: 1.2, pitch: 0, bearing: 0 };
const views: Record<string, Camera> = {
  // 85 km wide and 4.8 km deep, its central peak 2 km high, from the north
  tycho: { center: [-11.4, -42.6], zoom: 7.4, pitch: 62, bearing: 180 },
  // 93 km wide and 3.8 km deep, its terraced walls, from the south-east
  copernicus: { center: [-20.1, 9.0], zoom: 7.4, pitch: 62, bearing: -30 },
};
const BUTTONS: [view: string, label: string][] = [
  ['tycho', 'Tycho'],
  ['copernicus', 'Copernicus'],
  ['globe', 'Globe'],
];

// Where the lunar modules landed (longitude, latitude), and on which side of each its label goes
const landings: [name: string, lon: number, lat: number, west?: boolean][] = [
  ['Apollo 11', 23.473, 0.674],
  ['Apollo 12', -23.422, -3.012, true],
  ['Apollo 14', -17.471, -3.645],
  ['Apollo 15', 3.634, 26.132],
  ['Apollo 16', 15.5, -8.973],
  ['Apollo 17', 30.772, 20.191],
];
type Site = { name: string; west: boolean };
const sites: Feature<Site>[] = landings.map(([name, lon, lat, west = false]) => ({
  type: 'Feature',
  geometry: { type: 'Point', coordinates: [lon, lat] },
  properties: { name, west },
}));
// The labels' functions, made once (props are compared by identity)
const text = (p: Site) => p.name;
const anchor = (p: Site) => (p.west ? 'right' : 'left');
const offset = (p: Site): [number, number] => [p.west ? -0.6 : 0.6, 0];

const App = () => {
  const map = useRef<GeoMap>(undefined);
  const show = (view: string) => {
    history.replaceState(null, '', view === 'globe' ? location.pathname : `#${view}`);
    map.current?.flyTo(views[view] ?? globe, { duration: 5000 });
  };

  return (
    <>
      <IsardMap
        ref={map}
        id="map"
        body="moon"
        {...(views[location.hash.slice(1)] ?? globe)}
        maxPitch={75}
        // LOLA's model, credited by the map
        terrain
        onClick={({ lonLat }) => {
          if (lonLat) map.current?.flyTo({ center: lonLat, zoom: 7, pitch: 45 }, { duration: 4000 });
        }}
      >
        {/* The global mosaic, 100 m per pixel at the equator: levels 0 to 8 of an equirectangular grid */}
        <Layer
          type="raster"
          url="https://trek.nasa.gov/tiles/Moon/EQ/LRO_WAC_Mosaic_Global_303ppd_v02/1.0.0/default/default028mm/{z}/{y}/{x}.jpg"
          grid="equirectangular"
          maxZoom={8}
          attribution='<a href="https://trek.nasa.gov/moon/">NASA Moon Trek</a>: LRO WAC mosaic, NASA/GSFC/Arizona State University'
        />
        <Layer type="circles" data={sites} color="#ffcc33" radius={4} strokeColor="#1d2433" strokeWidth={1.5} />
        <Layer
          type="symbols"
          data={sites}
          text={text}
          size={12}
          color="#fff"
          haloColor="#1d2433"
          haloWidth={1.5}
          anchor={anchor}
          offset={offset}
        />
      </IsardMap>
      <div className="panel">
        <h1>The Moon</h1>
        <p>
          The LRO mosaic on a globe of the Moon's radius, over LOLA's relief, the Apollo landing sites on it. Click
          anywhere to fly there.
        </p>
        <p>
          {BUTTONS.map(([view, label]) => (
            <Fragment key={view}>
              <button type="button" onClick={() => show(view)}>
                {label}
              </button>{' '}
            </Fragment>
          ))}
        </p>
      </div>
    </>
  );
};

createRoot(element('root')).render(<App />);
