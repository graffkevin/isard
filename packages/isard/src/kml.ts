/**
 * KML to GeoJSON, for the feature layers: `map.addLines({ data: parseKml(await (await fetch(url)).text()) })`.
 */
import type { Feature, FeatureCollection, Geometry, Position, Properties } from './features.ts';
import { child, children, childText, descendants, parseXml, type XmlElement } from './xml.ts';

/** Positions of a `coordinates` element: `lon,lat[,alt]` tuples separated by spaces */
function positions(element: XmlElement | undefined): Position[] {
  const out: Position[] = [];
  for (const tuple of (element?.text ?? '').trim().split(/\s+/)) {
    const values = tuple.split(',').map(Number);
    if (values.length >= 2 && values.every(Number.isFinite)) out.push(values as Position);
  }
  return out;
}

/** The geometries of an element (a Placemark, a MultiGeometry), in order */
function geometries(parent: XmlElement): Geometry[] {
  const out: Geometry[] = [];
  for (const g of parent.children) {
    if (g.name === 'Point') {
      const [p] = positions(child(g, 'coordinates'));
      if (p) out.push({ type: 'Point', coordinates: p });
    } else if (g.name === 'LineString' || g.name === 'LinearRing') {
      const line = positions(child(g, 'coordinates'));
      if (line.length >= 2) out.push({ type: 'LineString', coordinates: line });
    } else if (g.name === 'Polygon') {
      const ring = (b: XmlElement) => positions(child(child(b, 'LinearRing'), 'coordinates'));
      const outer = child(g, 'outerBoundaryIs');
      const rings = [...(outer ? [ring(outer)] : []), ...children(g, 'innerBoundaryIs').map(ring)];
      if ((rings[0]?.length ?? 0) >= 4) out.push({ type: 'Polygon', coordinates: rings.filter((r) => r.length >= 4) });
    } else if (g.name === 'Track') {
      // gx:Track: its gx:coord, `lon lat alt`
      const line = children(g, 'coord')
        .map((c) => c.text.trim().split(/\s+/).map(Number))
        .filter((p) => p.length >= 2 && p.every(Number.isFinite)) as Position[];
      if (line.length >= 2) out.push({ type: 'LineString', coordinates: line });
    } else if (g.name === 'MultiGeometry' || g.name === 'MultiTrack') {
      out.push(...geometries(g));
    }
  }
  return out;
}

/** One geometry of several: a Multi… of one type, else a collection */
function merge(list: Geometry[]): Geometry | null {
  const [first] = list;
  if (!first) return null;
  if (list.length === 1) return first;
  const same = list.every((g) => g.type === first.type);
  if (same && first.type === 'Point') {
    return { type: 'MultiPoint', coordinates: list.map((g) => (g as { coordinates: Position }).coordinates) };
  }
  if (same && first.type === 'LineString') {
    return { type: 'MultiLineString', coordinates: list.map((g) => (g as { coordinates: Position[] }).coordinates) };
  }
  if (same && first.type === 'Polygon') {
    return { type: 'MultiPolygon', coordinates: list.map((g) => (g as { coordinates: Position[][] }).coordinates) };
  }
  return { type: 'GeometryCollection', geometries: list };
}

/** A KML color (`aabbggrr`, hexadecimal) as a CSS color and an opacity */
function color(element: XmlElement | undefined): [css: string, opacity: number] | undefined {
  const value = childText(element, 'color')?.replace(/^#/, '');
  if (!value || !/^[\da-f]{8}$/i.test(value)) return undefined;
  const [a, b, g, r] = [0, 2, 4, 6].map((i) => value.slice(i, i + 2));
  return [`#${r}${g}${b}`.toLowerCase(), Math.round((Number.parseInt(a as string, 16) / 255) * 1000) / 1000];
}

/** The properties of a style (simplestyle's names, as togeojson): stroke, stroke-opacity, stroke-width, fill,
 * fill-opacity, marker-color */
function styleProperties(style: XmlElement | undefined): Properties {
  const out: Properties = {};
  if (!style) return out;
  const line = child(style, 'LineStyle');
  const stroke = color(line);
  if (stroke) [out.stroke, out['stroke-opacity']] = stroke;
  const width = Number(childText(line, 'width'));
  if (childText(line, 'width') !== undefined && Number.isFinite(width)) out['stroke-width'] = width;
  const poly = child(style, 'PolyStyle');
  const fill = color(poly);
  if (fill) [out.fill, out['fill-opacity']] = fill;
  if (childText(poly, 'fill') === '0') out['fill-opacity'] = 0;
  if (childText(poly, 'outline') === '0') out['stroke-opacity'] = 0;
  const icon = color(child(style, 'IconStyle'));
  if (icon) out['marker-color'] = icon[0];
  return out;
}

/**
 * The placemarks of a KML document as GeoJSON features: their points, lines (and gx:Track), polygons (with their
 * holes) and MultiGeometry (a Multi… of one type, else a GeometryCollection); their `name`, `description`, extended
 * data (`Data`, `SimpleData`) as properties, and the colors of their style (inline, or shared through `styleUrl` and
 * `StyleMap`) as `stroke`, `stroke-opacity`, `stroke-width`, `fill`, `fill-opacity`, `marker-color`. A placemark
 * without geometry has a null one
 */
export function parseKml<P = Properties>(text: string): FeatureCollection<P> {
  const root = parseXml(text);
  const styles = new Map<string, XmlElement>();
  for (const s of descendants(root, 'Style')) if (s.attributes.id) styles.set(s.attributes.id, s);
  const maps = new Map<string, string>();
  for (const m of descendants(root, 'StyleMap')) {
    const normal = children(m, 'Pair').find((p) => childText(p, 'key') === 'normal');
    const url = childText(normal, 'styleUrl');
    if (m.attributes.id && url) maps.set(m.attributes.id, url);
  }
  /** The style of a `styleUrl` in this document (`#id`, through a StyleMap) */
  const shared = (url: string | undefined, depth = 0): XmlElement | undefined => {
    const id = url?.slice(url.indexOf('#') + 1);
    if (id === undefined || depth > 4) return undefined;
    return styles.get(id) ?? shared(maps.get(id), depth + 1);
  };
  const features = descendants(root, 'Placemark').map((placemark): Feature<P> => {
    const properties: Properties = { ...styleProperties(shared(childText(placemark, 'styleUrl'))) };
    Object.assign(properties, styleProperties(child(placemark, 'Style')));
    for (const key of ['name', 'description', 'address', 'styleUrl']) {
      const value = childText(placemark, key);
      if (value !== undefined) properties[key] = value;
    }
    const time = childText(child(placemark, 'TimeStamp'), 'when');
    if (time !== undefined) properties.time = time;
    const extended = child(placemark, 'ExtendedData');
    for (const data of children(extended, 'Data')) {
      if (data.attributes.name) properties[data.attributes.name] = childText(data, 'value') ?? '';
    }
    for (const schema of children(extended, 'SchemaData')) {
      for (const data of children(schema, 'SimpleData')) {
        if (data.attributes.name) properties[data.attributes.name] = data.text.trim();
      }
    }
    const id = placemark.attributes.id;
    return {
      type: 'Feature',
      geometry: merge(geometries(placemark)),
      properties: properties as P,
      ...(id === undefined ? {} : { id }),
    };
  });
  return { type: 'FeatureCollection', features };
}
