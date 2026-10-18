/**
 * `isard/formats`: KML, GPX and TopoJSON read into GeoJSON feature collections, for the feature layers.
 *
 * ```ts
 * import { parseGpx } from 'isard/formats';
 * map.addLines({ data: parseGpx(await (await fetch('track.gpx')).text()), color: '#0057b8' });
 * ```
 */
export { parseGpx } from './gpx.ts';
export { parseKml } from './kml.ts';
export { parseTopoJson, type TopoGeometry, type Topology } from './topojson.ts';
