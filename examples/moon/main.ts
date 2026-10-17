/**
 * The Moon: a map of another body. The globe takes the Moon's radius (no atmosphere, no Géoplateforme basemap), the
 * Lunar Reconnaissance Orbiter's wide-angle mosaic comes from NASA's Moon Trek in longitudes and latitudes, reprojected
 * in the page into Web Mercator tiles; the Apollo landing sites as circles and labels. A click flies to the point.
 */
import { type Feature, GeoMap } from '@ign-junn/isard';
import { element } from '../shared/wfs.ts';

const map = await GeoMap.create(element('map'), {
  body: 'moon',
  center: [5, 0],
  zoom: 1.2,
  attributionControl: {
    customAttribution:
      '<a href="https://trek.nasa.gov/moon/">NASA Moon Trek</a>: LRO WAC mosaic, NASA/GSFC/Arizona State University',
  },
});

// The global mosaic, 100 m per pixel at the equator: levels 0 to 8 of an equirectangular grid
map.addRaster({
  url: 'https://trek.nasa.gov/tiles/Moon/EQ/LRO_WAC_Mosaic_Global_303ppd_v02/1.0.0/default/default028mm/{z}/{y}/{x}.jpg',
  grid: 'equirectangular',
  maxZoom: 8,
});

// Where the lunar modules landed (longitude, latitude), and on which side of each its label goes
const landings: [name: string, lon: number, lat: number, west?: boolean][] = [
  ['Apollo 11', 23.473, 0.674],
  ['Apollo 12', -23.422, -3.012, true],
  ['Apollo 14', -17.471, -3.645],
  ['Apollo 15', 3.634, 26.132],
  ['Apollo 16', 15.5, -8.973],
  ['Apollo 17', 30.772, 20.191],
];
const sites: Feature<{ name: string; west: boolean }>[] = landings.map(([name, lon, lat, west = false]) => ({
  type: 'Feature',
  geometry: { type: 'Point', coordinates: [lon, lat] },
  properties: { name, west },
}));
map.addCircles({ data: sites, color: '#ffcc33', radius: 4, strokeColor: '#1d2433', strokeWidth: 1.5 });
map.addSymbols({
  data: sites,
  text: (p) => p.name,
  size: 12,
  color: '#fff',
  haloColor: '#1d2433',
  haloWidth: 1.5,
  anchor: (p) => (p.west ? 'right' : 'left'),
  offset: (p) => [p.west ? -0.6 : 0.6, 0],
});

map.on('click', ({ lonLat }) => {
  if (lonLat) map.flyTo({ center: lonLat, zoom: 7, pitch: 30 }, { duration: 4000 });
});
