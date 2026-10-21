/**
 * GPX to GeoJSON, for the feature layers: `map.addLines({ data: parseGpx(await (await fetch(url)).text()) })`.
 */
import type { Feature, FeatureCollection, Geometry, Position, Properties } from '../layers/features.ts';
import { child, children, childText, parseXml, type XmlElement } from './xml.ts';

/** The fields of a waypoint, route or track kept as properties when present */
const FIELDS = ['name', 'cmt', 'desc', 'src', 'sym', 'type', 'number'];

const properties = (element: XmlElement): Properties => {
  const out: Properties = {};

  FIELDS.forEach((key) => {
    const value = childText(element, key);

    if (value !== undefined) out[key] = key === 'number' ? Number(value) : value;
  });

  const link = child(element, 'link')?.attributes.href;

  if (link !== undefined) out.link = link;

  return out;
};

/** A point of a GPX file (wpt, rtept, trkpt): its position, its altitude third if any, and its time */
const point = (element: XmlElement): { position: Position; time: string | undefined } | undefined => {
  const lon = Number(element.attributes.lon);
  const lat = Number(element.attributes.lat);

  if (!Number.isFinite(lon) || !Number.isFinite(lat) || element.attributes.lon === undefined) return undefined;

  const ele = childText(element, 'ele');
  const position: Position = ele !== undefined && Number.isFinite(Number(ele)) ? [lon, lat, Number(ele)] : [lon, lat];

  return { position, time: childText(element, 'time') };
};

/** The positions of a line of points, and their times when every point has one */
const line = (points: XmlElement[]): { coordinates: Position[]; times: string[] | undefined } => {
  const read = points.map(point).filter((p) => p !== undefined);
  const times = read.map((p) => p.time).filter((t) => t !== undefined);

  return {
    coordinates: read.map((p) => p.position),
    times: read.length > 0 && times.length === read.length ? times : undefined,
  };
};

/**
 * The waypoints, routes and tracks of a GPX file as GeoJSON features: a waypoint as a point (its `time` a property),
 * a route as a line, a track as a line or, of several segments, a multi-line; altitudes (`ele`) as the third
 * coordinate; `name`, `desc`, `cmt`, `type`… as properties, and the times of the points as `times` (one list, or one
 * per segment) when they all have one
 */
export const parseGpx = <P = Properties>(text: string): FeatureCollection<P> => {
  const root = parseXml(text);
  const features: Feature<P>[] = [];
  const add = (geometry: Geometry, props: Properties) =>
    features.push({ type: 'Feature', geometry, properties: props as P });

  children(root, 'wpt').forEach((wpt) => {
    const p = point(wpt);

    if (!p) return;
    add({ type: 'Point', coordinates: p.position }, { ...properties(wpt), ...(p.time ? { time: p.time } : {}) });
  });
  children(root, 'rte').forEach((rte) => {
    const { coordinates, times } = line(children(rte, 'rtept'));

    if (coordinates.length >= 2)
      add({ type: 'LineString', coordinates }, { ...properties(rte), ...(times ? { times } : {}) });
  });
  children(root, 'trk').forEach((trk) => {
    const segments = children(trk, 'trkseg')
      .map((s) => line(children(s, 'trkpt')))
      .filter((s) => s.coordinates.length >= 2);
    const [first] = segments;

    if (!first) return;

    const props = properties(trk);

    if (segments.length === 1) {
      add(
        { type: 'LineString', coordinates: first.coordinates },
        { ...props, ...(first.times ? { times: first.times } : {}) },
      );
    } else {
      const times = segments.every((s) => s.times) ? { times: segments.map((s) => s.times) } : {};

      add({ type: 'MultiLineString', coordinates: segments.map((s) => s.coordinates) }, { ...props, ...times });
    }
  });

  return { type: 'FeatureCollection', features };
};
