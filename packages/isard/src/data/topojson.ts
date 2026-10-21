/**
 * TopoJSON to GeoJSON, for the feature layers: `map.addFills({ data: parseTopoJson(topology, 'counties') })`.
 */
import type { Feature, FeatureCollection, Geometry, Position, Properties } from '../layers/features.ts';

/** A TopoJSON geometry object (arcs by index, `~i` for arc i reversed; positions quantized with a transform) */
export interface TopoGeometry {
  type:
    | 'Point'
    | 'MultiPoint'
    | 'LineString'
    | 'MultiLineString'
    | 'Polygon'
    | 'MultiPolygon'
    | 'GeometryCollection'
    | null;
  id?: string | number;
  properties?: Properties;
  coordinates?: number[] | number[][];
  arcs?: number[] | number[][] | number[][][];
  geometries?: TopoGeometry[];
}

/** A TopoJSON topology */
export interface Topology {
  type: 'Topology';
  /** Quantized positions: x × scale + translate (arcs delta-encoded) */
  transform?: { scale: [number, number]; translate: [number, number] };
  arcs: number[][][];
  objects: Record<string, TopoGeometry>;
}

/**
 * The features of object `name` of a TopoJSON topology (default its first object): a GeometryCollection gives one
 * feature per geometry, another geometry one feature; arcs are stitched and dequantized, ids and properties kept
 */
export const parseTopoJson = <P = Properties>(topology: Topology, name?: string): FeatureCollection<P> => {
  const object = name === undefined ? Object.values(topology.objects)[0] : topology.objects[name];

  if (!object) throw new Error(`isard: no TopoJSON object ${name ?? ''}`);

  const { scale = [1, 1], translate = [0, 0] } = topology.transform ?? {};
  const quantized = topology.transform !== undefined;
  const at = (p: number[]): Position => [p[0] ?? 0, p[1] ?? 0, ...p.slice(2)];
  // The arcs decoded once
  const arcs = topology.arcs.map((arc) => {
    let [x, y] = [0, 0];

    return arc.map((p): Position => {
      if (!quantized) return at(p);
      x += p[0] ?? 0;
      y += p[1] ?? 0;

      return [x * scale[0] + translate[0], y * scale[1] + translate[1], ...p.slice(2)];
    });
  });
  const position = (p: number[]): Position =>
    quantized ? [(p[0] ?? 0) * scale[0] + translate[0], (p[1] ?? 0) * scale[1] + translate[1], ...p.slice(2)] : at(p);
  /** A line from arcs, each after the first without its first point (the last of the one before) */
  const stitch = (indices: number[]): Position[] => {
    const out: Position[] = [];

    indices.forEach((i) => {
      const arc = arcs[i < 0 ? ~i : i] ?? [];
      const points = i < 0 ? [...arc].reverse() : arc;

      out.push(...(out.length > 0 ? points.slice(1) : points));
    });

    return out;
  };
  const geometry = (g: TopoGeometry): Geometry | null => {
    switch (g.type) {
      case 'Point':
        return { type: 'Point', coordinates: position(g.coordinates as number[]) };
      case 'MultiPoint':
        return { type: 'MultiPoint', coordinates: (g.coordinates as number[][]).map(position) };
      case 'LineString':
        return { type: 'LineString', coordinates: stitch(g.arcs as number[]) };
      case 'MultiLineString':
        return { type: 'MultiLineString', coordinates: (g.arcs as number[][]).map(stitch) };
      case 'Polygon':
        return { type: 'Polygon', coordinates: (g.arcs as number[][]).map(stitch) };
      case 'MultiPolygon':
        return { type: 'MultiPolygon', coordinates: (g.arcs as number[][][]).map((p) => p.map(stitch)) };
      case 'GeometryCollection':
        return {
          type: 'GeometryCollection',
          geometries: (g.geometries ?? []).map(geometry).filter((c) => c !== null),
        };
      default:
        return null;
    }
  };
  const feature = (g: TopoGeometry): Feature<P> => ({
    type: 'Feature',
    geometry: geometry(g),
    properties: (g.properties ?? {}) as P,
    ...(g.id === undefined ? {} : { id: g.id }),
  });
  const features = object.type === 'GeometryCollection' ? (object.geometries ?? []).map(feature) : [feature(object)];

  return { type: 'FeatureCollection', features };
};
