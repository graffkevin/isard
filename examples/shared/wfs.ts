/**
 * Features of the Géoplateforme's WFS (BD TOPO, ADMIN EXPRESS…) in a box, as GeoJSON: the data of the examples, no
 * key needed.
 */
import type { FeatureCollection, Properties } from '@ign-junn/isard';

export async function wfs<P = Properties>(
  layer: string,
  [west, south, east, north]: [number, number, number, number],
  count = 5000,
): Promise<FeatureCollection<P>> {
  const url = new URL('https://data.geopf.fr/wfs/ows');
  url.search = new URLSearchParams({
    SERVICE: 'WFS',
    VERSION: '2.0.0',
    REQUEST: 'GetFeature',
    TYPENAMES: layer,
    OUTPUTFORMAT: 'application/json',
    SRSNAME: 'EPSG:4326',
    // WFS 2.0 in EPSG:4326: latitude first
    BBOX: `${south},${west},${north},${east},urn:ogc:def:crs:EPSG::4326`,
    COUNT: String(count),
  }).toString();
  const response = await fetch(url);
  if (!response.ok) throw new Error(`${layer}: ${response.status}`);
  return response.json();
}

/** The element of an id, typed */
export function element<T extends HTMLElement = HTMLElement>(id: string): T {
  const found = document.getElementById(id);
  if (!found) throw new Error(`#${id} missing from the page`);
  return found as T;
}
