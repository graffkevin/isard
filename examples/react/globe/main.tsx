/**
 * The globe in React: the IGN orthophotos (Géoplateforme; worldwide imagery at low zoom) on a sphere among the stars,
 * turning slowly until the map is touched; a click flies to the point (van Wijk and Nuij's path, as MapLibre's flyTo),
 * the sphere blending into Web Mercator on the way. The arrows go to the Moon and Mars: the body is set when a map is
 * created, so each is a new map in the same element (a new `key`).
 */
import type { CelestialBody } from 'isard';
import { IsardMap, type IsardMapProps, Layer, useMap, useMapEvent } from 'isard/react';
import { useEffect, useRef, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { element } from '../../shared/page.ts';

interface Body {
  body: CelestialBody;
  name: string;
  text: string;
  /** Longitude facing the camera at first */
  lon: number;
  /** An equirectangular mosaic from NASA's Trek, for the bodies without a basemap */
  mosaic?: { url: string; maxZoom: number; attribution: string };
}

const EARTH: Body = {
  body: 'earth',
  name: 'The Earth',
  text: 'The Earth turning in its atmosphere. Click anywhere to fly there: the globe turns into Web Mercator as you zoom in.',
  lon: 2.5,
};
const BODIES: Body[] = [
  EARTH,
  {
    body: 'moon',
    name: 'The Moon',
    text: "The LRO mosaic on a globe of the Moon's radius, no atmosphere around it. Click anywhere to fly there.",
    lon: 5,
    mosaic: {
      url: 'https://trek.nasa.gov/tiles/Moon/EQ/LRO_WAC_Mosaic_Global_303ppd_v02/1.0.0/default/default028mm/{z}/{y}/{x}.jpg',
      maxZoom: 8,
      attribution:
        '<a href="https://trek.nasa.gov/moon/">NASA Moon Trek</a>: LRO WAC mosaic, NASA/GSFC/Arizona State University',
    },
  },
  {
    body: 'mars',
    name: 'Mars',
    text: 'The Viking colour mosaic on a globe of Mars, in its thin dusty halo. Click anywhere to fly there.',
    lon: -80,
    mosaic: {
      url: 'https://trek.nasa.gov/tiles/Mars/EQ/Mars_Viking_MDIM21_ClrMosaic_global_232m/1.0.0/default/default028mm/{z}/{y}/{x}.jpg',
      maxZoom: 7,
      attribution:
        '<a href="https://trek.nasa.gov/mars/">NASA Mars Trek</a>: Viking MDIM 2.1 colour mosaic, NASA/JPL-Caltech/USGS',
    },
  },
];

/** Turns the map around a few degrees a second until the first gesture; a click flies to the point */
const Spin = ({ zoom }: { zoom: number }) => {
  const map = useMap();
  const turning = useRef(true);

  useEffect(() => {
    if (!map) return;

    let last = performance.now();
    let frame = 0;
    const turn = (now: number) => {
      if (turning.current) {
        const { center } = map.getCamera();

        map.setCamera({ center: [center[0] + ((now - last) / 1000) * 4, center[1]] });
      }
      last = now;
      frame = requestAnimationFrame(turn);
    };

    frame = requestAnimationFrame(turn);

    const stop = () => {
      turning.current = false;
    };
    const canvas = map.getCanvas();

    canvas.addEventListener('pointerdown', stop);

    return () => {
      cancelAnimationFrame(frame);
      canvas.removeEventListener('pointerdown', stop);
    };
  }, [map]);

  useMapEvent('click', ({ lonLat }) => {
    turning.current = false;
    if (lonLat) map?.flyTo({ center: lonLat, zoom, pitch: 45 }, { duration: 5000 });
  });

  return null;
};

const App = () => {
  const [index, setIndex] = useState(0);
  const { body, name, text, lon, mosaic } = BODIES[index] ?? EARTH;
  const earth = body === 'earth';
  // The Earth has its basemap; the other bodies their mosaic
  const basemap: Partial<IsardMapProps> = earth ? { basemap: 'ign-ortho' } : {};
  const step = (by: number) => setIndex((i) => (i + by + BODIES.length) % BODIES.length);

  return (
    <>
      <IsardMap
        key={name}
        id="map"
        body={body}
        center={[lon, earth ? 30 : 0]}
        zoom={earth ? 1.4 : 1.2}
        projection="globe"
        {...basemap}
      >
        {mosaic && <Layer type="raster" {...mosaic} grid="equirectangular" />}
        <Spin zoom={earth ? 12 : 6} />
      </IsardMap>
      <div className="panel">
        <h1>{name}</h1>
        <p>{text}</p>
        <p>The arrows go to the Earth, the Moon and Mars.</p>
      </div>
      <button type="button" className="arrow" id="previous" aria-label="Previous body" onClick={() => step(-1)}>
        ‹
      </button>
      <button type="button" className="arrow" id="next" aria-label="Next body" onClick={() => step(1)}>
        ›
      </button>
    </>
  );
};

createRoot(element('root')).render(<App />);
