/**
 * The globe: the IGN orthophotos (Géoplateforme; worldwide imagery at low zoom) on a sphere among the stars, turning slowly until the map is touched; a click flies to
 * the point (van Wijk and Nuij's path, as MapLibre's flyTo), the sphere blending into Web Mercator on the way. The
 * arrows go to the Moon and Mars: the body is set when a map is created, so each is a new map in the same element.
 */
import { type CelestialBody, GeoMap } from 'isard';
import { element } from '../shared/page.ts';

interface Body {
  body: CelestialBody;
  name: string;
  text: string;
  /** Longitude facing the camera at first */
  lon: number;
  /** An equirectangular mosaic from NASA's Trek, for the bodies without a basemap */
  mosaic?: { url: string; maxZoom: number; attribution: string };
}

const bodies: Body[] = [
  {
    body: 'earth',
    name: 'The Earth',
    text: 'The Earth turning in its atmosphere. Click anywhere to fly there: the globe turns into Web Mercator as you zoom in.',
    lon: 2.5,
  },
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

const container = element('map');
let map: GeoMap | undefined;
let turning = true;
/** Counts the switches: a map created after a newer switch is thrown away */
let shown = 0;

// Turns a few degrees a second until the first gesture
let last = performance.now();
const turn = (now: number) => {
  if (turning && map) {
    const { center } = map.getCamera();

    map.setCamera({ center: [center[0] + ((now - last) / 1000) * 4, center[1]] });
  }
  last = now;
  requestAnimationFrame(turn);
};
requestAnimationFrame(turn);

const show = async (index: number) => {
  const shownBody = bodies[index];

  if (!shownBody) return;

  const ticket = ++shown;
  const { body, name, text, lon, mosaic } = shownBody;

  element('name').textContent = name;
  element('text').textContent = text;
  map?.destroy();
  map = undefined;

  const created = await GeoMap.create(container, {
    body,
    center: [lon, body === 'earth' ? 30 : 0],
    zoom: body === 'earth' ? 1.4 : 1.2,
    projection: 'globe',
    ...(body === 'earth' && { basemap: 'ign-ortho' as const }),
  });

  if (ticket !== shown) {
    created.destroy();

    return;
  }
  if (mosaic) created.addRaster({ ...mosaic, grid: 'equirectangular' });
  created.getCanvas().addEventListener('pointerdown', () => {
    turning = false;
  });
  created.on('click', ({ lonLat }) => {
    turning = false;
    if (lonLat) created.flyTo({ center: lonLat, zoom: body === 'earth' ? 12 : 6, pitch: 45 }, { duration: 5000 });
  });
  map = created;
  turning = true;
};

let current = 0;
(
  [
    ['previous', -1],
    ['next', 1],
  ] as const
).forEach(([id, step]) => {
  element(id).addEventListener('click', () => {
    current = (current + step + bodies.length) % bodies.length;
    void show(current);
  });
});
await show(current);
