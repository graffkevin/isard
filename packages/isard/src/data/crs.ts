/**
 * Coordinate reference systems of the data: a CRS turns longitudes and latitudes (degrees) into its coordinates
 * (`forward`) and back (`inverse`). The map stays in Web Mercator (or the globe): data in another CRS is reprojected
 * into it, as rasters (see `TileGrid`) or as GeoJSON (`reproject`).
 *
 * Built in, exact to the millimetre against PROJ: EPSG:4326 (and the geographic ETRS89, RGF93), EPSG:3857, Lambert-93
 * (EPSG:2154), the nine conic zones CC42 to CC50 (EPSG:3942 to 3950), the UTM zones of WGS 84 (EPSG:32601 to 32660
 * north, 32701 to 32760 south) and of ETRS89 (EPSG:25828 to 25838). WGS 84, ETRS89 and RGF93 are taken as one datum:
 * they differ by less than a metre. Any other CRS is plugged in as an object with `forward` and `inverse`, such as
 * `proj4('EPSG:27572')`, directly or by name through `registerCrs`.
 */
import type { Feature, FeatureCollection, FeatureData, Geometry, Position } from '../layers/features.ts';

/** A CRS: longitudes and latitudes (degrees) to its coordinates (x east, y north), and back. A converter of proj4
 * (`proj4('EPSG:2154')`) is one */
export interface Crs {
  forward(lonLat: [number, number]): readonly number[];
  inverse(xy: [number, number]): readonly number[];
}

/** A CRS by name (`'EPSG:2154'`, `'urn:ogc:def:crs:EPSG::2154'`, `'CRS:84'`…; built in or registered), or itself */
export type CrsLike = string | Crs;

/** An ellipsoid: semi-major axis (metres) and flattening */
type Ellipsoid = [a: number, f: number];
const GRS80: Ellipsoid = [6378137, 1 / 298.257222101];
const WGS84: Ellipsoid = [6378137, 1 / 298.257223563];

const RAD = Math.PI / 180;

/** Isometric latitude of a latitude (radians) on an ellipsoid of eccentricity `e` */
const isometric = (phi: number, e: number) => Math.atanh(Math.sin(phi)) - e * Math.atanh(e * Math.sin(phi));

/** Latitude (radians) of an isometric latitude, by fixed-point iteration (converges by e² at each step) */
function fromIsometric(psi: number, e: number): number {
  let phi = Math.asin(Math.tanh(psi));
  for (let i = 0; i < 20; i += 1) {
    const next = Math.asin(Math.tanh(psi + e * Math.atanh(e * Math.sin(phi))));
    if (Math.abs(next - phi) < 1e-15) return next;
    phi = next;
  }
  return phi;
}

/** An angle (radians) brought into [-π, π] */
const wrap = (angle: number) => angle - 2 * Math.PI * Math.round(angle / (2 * Math.PI));

/**
 * Lambert Conformal Conic with two standard parallels (EPSG method 9802), angles in degrees: the projection of
 * Lambert-93 and of the CC zones, and of many national grids
 */
export function lambertConformalConic(p: {
  /** Semi-major axis (metres) and flattening (default GRS80) */
  ellipsoid?: [a: number, f: number];
  lat0: number;
  lon0: number;
  lat1: number;
  lat2: number;
  x0: number;
  y0: number;
}): Crs {
  const [a, f] = p.ellipsoid ?? GRS80;
  const e = Math.sqrt(f * (2 - f));
  const m = (phi: number) => Math.cos(phi) / Math.sqrt(1 - (e * Math.sin(phi)) ** 2);
  const [phi1, phi2] = [p.lat1 * RAD, p.lat2 * RAD];
  const n =
    phi1 === phi2
      ? Math.sin(phi1)
      : (Math.log(m(phi1)) - Math.log(m(phi2))) / (isometric(phi2, e) - isometric(phi1, e));
  const aF = (a * m(phi1) * Math.exp(n * isometric(phi1, e))) / n;
  const r0 = aF * Math.exp(-n * isometric(p.lat0 * RAD, e));
  const lon0 = p.lon0 * RAD;
  return {
    forward: ([lon, lat]) => {
      const r = aF * Math.exp(-n * isometric(lat * RAD, e));
      const theta = n * wrap(lon * RAD - lon0);
      return [p.x0 + r * Math.sin(theta), p.y0 + r0 - r * Math.cos(theta)];
    },
    inverse: ([x, y]) => {
      const s = Math.sign(n);
      const [dx, dy] = [x - p.x0, r0 - (y - p.y0)];
      const r = s * Math.hypot(dx, dy);
      const theta = Math.atan2(s * dx, s * dy);
      return [(theta / n + lon0) / RAD, fromIsometric(-Math.log(r / aF) / n, e) / RAD];
    },
  };
}

/**
 * Transverse Mercator (EPSG method 9807) by Krüger's series to the sixth order (Karney, 2011), angles in degrees:
 * exact to the tenth of a millimetre within 4000 km of the central meridian. The projection of the UTM zones
 */
export function transverseMercator(p: {
  /** Semi-major axis (metres) and flattening (default GRS80) */
  ellipsoid?: [a: number, f: number];
  lon0: number;
  /** Scale on the central meridian */
  k0: number;
  x0: number;
  y0: number;
}): Crs {
  const [a, f] = p.ellipsoid ?? GRS80;
  const e = Math.sqrt(f * (2 - f));
  const n = f / (2 - f);
  const [n2, n3, n4, n5, n6] = [n ** 2, n ** 3, n ** 4, n ** 5, n ** 6];
  const kA = (p.k0 * a * (1 + n2 / 4 + n4 / 64 + n6 / 256)) / (1 + n);
  const alpha = [
    n / 2 - (2 * n2) / 3 + (5 * n3) / 16 + (41 * n4) / 180 - (127 * n5) / 288 + (7891 * n6) / 37800,
    (13 * n2) / 48 - (3 * n3) / 5 + (557 * n4) / 1440 + (281 * n5) / 630 - (1983433 * n6) / 1935360,
    (61 * n3) / 240 - (103 * n4) / 140 + (15061 * n5) / 26880 + (167603 * n6) / 181440,
    (49561 * n4) / 161280 - (179 * n5) / 168 + (6601661 * n6) / 7257600,
    (34729 * n5) / 80640 - (3418889 * n6) / 1995840,
    (212378941 * n6) / 319334400,
  ];
  const beta = [
    n / 2 - (2 * n2) / 3 + (37 * n3) / 96 - n4 / 360 - (81 * n5) / 512 + (96199 * n6) / 604800,
    n2 / 48 + n3 / 15 - (437 * n4) / 1440 + (46 * n5) / 105 - (1118711 * n6) / 3870720,
    (17 * n3) / 480 - (37 * n4) / 840 - (209 * n5) / 4480 + (5569 * n6) / 90720,
    (4397 * n4) / 161280 - (11 * n5) / 504 - (830251 * n6) / 7257600,
    (4583 * n5) / 161280 - (108847 * n6) / 3991680,
    (20648693 * n6) / 638668800,
  ];
  /** ξ, η moved by the series of coefficients `c`, added (forward) or subtracted (inverse) */
  const series = (xi: number, eta: number, c: number[], sign: number): [number, number] => {
    let [x, y] = [xi, eta];
    c.forEach((k, j) => {
      const w = 2 * (j + 1);
      x += sign * k * Math.sin(w * xi) * Math.cosh(w * eta);
      y += sign * k * Math.cos(w * xi) * Math.sinh(w * eta);
    });
    return [x, y];
  };
  const lon0 = p.lon0 * RAD;
  return {
    forward: ([lon, lat]) => {
      const lambda = wrap(lon * RAD - lon0);
      // The conformal latitude's tangent
      const t = Math.sinh(isometric(lat * RAD, e));
      const [xi, eta] = series(
        Math.atan2(t, Math.cos(lambda)),
        Math.asinh(Math.sin(lambda) / Math.hypot(t, Math.cos(lambda))),
        alpha,
        1,
      );
      return [p.x0 + kA * eta, p.y0 + kA * xi];
    },
    inverse: ([x, y]) => {
      const [xi, eta] = series((y - p.y0) / kA, (x - p.x0) / kA, beta, -1);
      const t = Math.sin(xi) / Math.hypot(Math.sinh(eta), Math.cos(xi));
      return [(lon0 + Math.atan2(Math.sinh(eta), Math.cos(xi))) / RAD, fromIsometric(Math.asinh(t), e) / RAD];
    },
  };
}

/** Longitudes and latitudes themselves */
const GEOGRAPHIC: Crs = {
  forward: ([lon, lat]) => [lon, lat],
  inverse: ([x, y]) => [x, y],
};

/** Web Mercator in metres (EPSG:3857) */
const R = 6378137;
const WEB_MERCATOR: Crs = {
  forward: ([lon, lat]) => [R * lon * RAD, R * Math.log(Math.tan(Math.PI / 4 + (lat * RAD) / 2))],
  inverse: ([x, y]) => [x / R / RAD, (2 * Math.atan(Math.exp(y / R)) - Math.PI / 2) / RAD],
};

/** The CRSs registered by the application, by name */
const registered = new Map<string, Crs>();
/** The built-in CRSs made so far, by EPSG code */
const made = new Map<number, Crs>();

/**
 * The EPSG code of a CRS name: `EPSG:2154`, `urn:ogc:def:crs:EPSG::2154`, `urn:ogc:def:crs:EPSG:6.3:2154`,
 * `http://www.opengis.net/def/crs/EPSG/0/2154`; `CRS:84`, `OGC:CRS84` and the IGN's `IGNF:WGS84G` are 4326, its
 * `IGNF:LAMB93` 2154, the old names of Web Mercator 3857. Undefined for another name
 */
export function epsgCode(name: string): number | undefined {
  const n = name.trim().toUpperCase();
  if (/CRS:?84$/.test(n) || n === 'IGNF:WGS84G') return 4326;
  if (n === 'IGNF:LAMB93') return 2154;
  const code = /EPSG(?::+|\/)(?:[\d.]+[:/]+)?(\d+)$/.exec(n)?.[1];
  if (code === undefined) return undefined;
  const c = Number(code);
  return c === 900913 || c === 3785 || c === 102100 || c === 102113 ? 3857 : c;
}

/** A built-in CRS by EPSG code; undefined if not built in */
function builtin(code: number): Crs | undefined {
  if (code === 4326 || code === 4258 || code === 4171) return GEOGRAPHIC;
  if (code === 3857) return WEB_MERCATOR;
  if (code === 2154) {
    return lambertConformalConic({ lat0: 46.5, lon0: 3, lat1: 49, lat2: 44, x0: 700000, y0: 6600000 });
  }
  if (code >= 3942 && code <= 3950) {
    const lat0 = code - 3900;
    const y0 = (lat0 - 41) * 1000000 + 200000;
    return lambertConformalConic({ lat0, lon0: 3, lat1: lat0 - 0.75, lat2: lat0 + 0.75, x0: 1700000, y0 });
  }
  const utm = (zone: number, south: boolean, ellipsoid: Ellipsoid) =>
    transverseMercator({ ellipsoid, lon0: zone * 6 - 183, k0: 0.9996, x0: 500000, y0: south ? 10000000 : 0 });
  if (code >= 32601 && code <= 32660) return utm(code - 32600, false, WGS84);
  if (code >= 32701 && code <= 32760) return utm(code - 32700, true, WGS84);
  if (code >= 25828 && code <= 25838) return utm(code - 25800, false, GRS80);
  return undefined;
}

/** Whether a CRS name is longitudes and latitudes (needs no reprojection) */
export function isGeographic(name: string): boolean {
  const code = epsgCode(name);
  return !registered.has(normalize(name)) && code !== undefined && builtin(code) === GEOGRAPHIC;
}

const normalize = (name: string) => {
  const code = epsgCode(name);
  return code === undefined ? name.trim().toUpperCase() : `EPSG:${code}`;
};

/**
 * Names a CRS for every place that takes one (`registerCrs('EPSG:27572', proj4('EPSG:27572'))`): the `crs` of a tile
 * grid, the `crs` member of GeoJSON, `reproject`. A registered name wins over a built-in one
 */
export function registerCrs(name: string, crs: Crs): void {
  registered.set(normalize(name), crs);
}

/** The CRS of a name (registered or built in), or the CRS given; throws for an unknown name */
export function getCrs(crs: CrsLike): Crs {
  if (typeof crs !== 'string') return crs;
  const found = registered.get(normalize(crs));
  if (found) return found;
  const code = epsgCode(crs);
  if (code !== undefined) {
    let c = made.get(code);
    if (!c) {
      c = builtin(code);
      if (c) made.set(code, c);
    }
    if (c) return c;
  }
  throw new Error(`isard: unknown CRS ${crs}: registerCrs('${crs}', proj4('${crs}')) names it`);
}

/** The name in the legacy `crs` member of GeoJSON (`{ type: 'name', properties: { name } }`), if any */
export function crsOf(data: unknown): string | undefined {
  const member = (data as { crs?: { properties?: { name?: unknown } } } | null)?.crs;
  const name = member?.properties?.name;
  return typeof name === 'string' ? name : undefined;
}

/** A geometry with its positions mapped */
function mapGeometry(g: Geometry, f: (p: Position) => Position): Geometry {
  switch (g.type) {
    case 'Point':
      return { type: g.type, coordinates: f(g.coordinates) };
    case 'MultiPoint':
    case 'LineString':
      return { type: g.type, coordinates: g.coordinates.map(f) };
    case 'MultiLineString':
    case 'Polygon':
      return { type: g.type, coordinates: g.coordinates.map((line) => line.map(f)) };
    case 'MultiPolygon':
      return { type: g.type, coordinates: g.coordinates.map((poly) => poly.map((line) => line.map(f))) };
    case 'GeometryCollection':
      return { type: g.type, geometries: g.geometries.map((child) => mapGeometry(child, f)) };
  }
}

/**
 * GeoJSON in `from` (default: the CRS its `crs` member names) as GeoJSON in longitudes and latitudes, for the feature
 * layers: `map.addFills({ data: reproject(parcels, 'EPSG:2154') })`. The features are copied with new geometries, their
 * properties shared; altitudes are kept. Data without a CRS, or in longitudes and latitudes, comes back as it is.
 * The feature layers do this by themselves for a collection whose `crs` member names its CRS (a WFS's output in
 * EPSG:2154)
 */
export function reproject<P>(data: FeatureData<P>, from?: CrsLike): FeatureCollection<P> {
  let features = Array.isArray(data) ? data : [data as Feature<P>];
  if (!Array.isArray(data) && data.type === 'FeatureCollection') features = data.features;
  const source = from ?? crsOf(data);
  if (source === undefined || (typeof source === 'string' && isGeographic(source))) {
    return { type: 'FeatureCollection', features };
  }
  const crs = getCrs(source);
  const f = (p: Position): Position => {
    const [lon, lat] = crs.inverse([p[0], p[1]]);
    return [lon ?? Number.NaN, lat ?? Number.NaN, ...p.slice(2)];
  };
  return {
    type: 'FeatureCollection',
    features: features.map(
      (feature): Feature<P> => ({
        ...feature,
        geometry: feature.geometry && mapGeometry(feature.geometry, f),
      }),
    ),
  };
}
