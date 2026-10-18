/**
 * Mars: a map of another body with an atmosphere. The globe takes Mars's radius and its thin dusty halo, the Viking
 * colour mosaic (MDIM 2.1) comes from NASA's Mars Trek in longitudes and latitudes, reprojected in the page into Web
 * Mercator tiles; famous sites as circles and labels. A click flies to the point.
 */
import { type Feature, GeoMap } from '@ign-junn/isard';
import { element } from '../shared/wfs.ts';

const map = await GeoMap.create(element('map'), {
  body: 'mars',
  // Tharsis and Valles Marineris
  center: [-80, 0],
  zoom: 1.2,
  attributionControl: {
    customAttribution:
      '<a href="https://trek.nasa.gov/mars/">NASA Mars Trek</a>: Viking MDIM 2.1 colour mosaic, NASA/JPL-Caltech/USGS',
  },
});

// The global mosaic, 232 m per pixel: levels 0 to 7 of an equirectangular grid
map.addRaster({
  url: 'https://trek.nasa.gov/tiles/Mars/EQ/Mars_Viking_MDIM21_ClrMosaic_global_232m/1.0.0/default/default028mm/{z}/{y}/{x}.jpg',
  grid: 'equirectangular',
  maxZoom: 7,
});

// Planetocentric latitudes, longitudes east from -180 to 180 (those of the mosaic), and on which side the label goes
const places: [name: string, lon: number, lat: number, west?: boolean][] = [
  ['Olympus Mons', -133.8, 18.65, true],
  ['Valles Marineris', -59.2, -13.9],
  ['Curiosity, Gale crater', 137.44, -4.59],
  ['Perseverance, Jezero', 77.45, 18.44],
  ['Hellas Planitia', 70.5, -42.4],
  ['Viking 1', -47.95, 22.27],
];
const sites: Feature<{ name: string; west: boolean }>[] = places.map(([name, lon, lat, west = false]) => ({
  type: 'Feature',
  geometry: { type: 'Point', coordinates: [lon, lat] },
  properties: { name, west },
}));
map.addCircles({ data: sites, color: '#8ce8ff', radius: 4, strokeColor: '#2a1610', strokeWidth: 1.5 });
map.addSymbols({
  data: sites,
  text: (p) => p.name,
  size: 12,
  color: '#fff',
  haloColor: '#2a1610',
  haloWidth: 1.5,
  anchor: (p) => (p.west ? 'right' : 'left'),
  offset: (p) => [p.west ? -0.6 : 0.6, 0],
});

map.on('click', ({ lonLat }) => {
  if (lonLat) map.flyTo({ center: lonLat, zoom: 7, pitch: 30 }, { duration: 4000 });
});
