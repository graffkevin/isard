import { describe, expect, test } from 'bun:test';
import { type Crs, epsgCode, getCrs, isGeographic, registerCrs, reproject } from '../../src/data/crs.ts';
import { type FeatureCollection, featuresOf } from '../../src/layers/features.ts';

/**
 * Reference points: EPSG code, longitude, latitude (degrees), x, y (metres), from PROJ 9.8 (`cs2cs EPSG:4326
 * EPSG:<code>`, to the tenth of a millimetre). Lambert-93's natural origin (3° E, 46.5° N) is (700 000, 6 600 000) by
 * definition (IGN).
 */
const REFERENCES: [number, number, number, number, number][] = [
  // Lambert-93: the Panthéon, the natural origin, Brest, Nice, Bastia, Strasbourg, Dunkirk, Bonifacio
  [2154, 2.3461, 48.8462, 652011.8817, 6860882.5655],
  [2154, 3, 46.5, 700000, 6600000],
  [2154, -4.4861, 48.3904, 146632.9785, 6836262.3267],
  [2154, 7.262, 43.7102, 1043410.1597, 6299400.043],
  [2154, 9.45, 42.7, 1228546.0453, 6199420.644],
  [2154, 7.7521, 48.5734, 1050362.6954, 6840899.6472],
  [2154, 2.3768, 51.0344, 656187.5173, 7104472.924],
  [2154, 9.1592, 41.3874, 1216149.8327, 6051869.1496],
  // CC zones: Perpignan (CC42), Toulouse (CC44), Lyon (CC46), Paris (CC49), Lille (CC50)
  [3942, 2.8948, 42.6887, 1691378.8304, 1276501.3741],
  [3944, 1.4442, 43.6047, 1574404.8784, 3157266.8519],
  [3946, 4.8357, 45.764, 1842779.1092, 5175416.4283],
  [3949, 2.3461, 48.8462, 1652010.3332, 8183104.2929],
  [3950, 3.0573, 50.6292, 1704054.1985, 9269986.1049],
  // UTM of WGS 84: Paris (31N), the equator on 15° E (33N), Lima (18S), near the antimeridian (1N), 6° off the central
  // meridian (31N), New Zealand (60S)
  [32631, 2.3461, 48.8462, 452025.1521, 5410564.926],
  [32633, 15, 0, 500000, 0],
  [32718, -77.0428, -12.0464, 277617.4532, 8667487.897],
  [32601, -179, 60, 388455.958, 6653097.4353],
  [32631, 9, 45, 972891.7913, 5000491.0055],
  [32760, 175, -45, 342369.3593, 5015103.8287],
  // UTM of ETRS89: Munich (32N), Madrid (30N), Las Palmas (28N), a central meridian (38N)
  [25832, 11.582, 48.1351, 692095.1452, 5334540.636],
  [25830, -3.7038, 40.4168, 440290.4581, 4474257.3819],
  [25828, -15.4363, 28.1235, 457150.659, 3110960.1251],
  [25838, 45, 55.75, 500000, 6178255.8326],
];

describe('built-in CRSs', () => {
  test.each(REFERENCES)('EPSG:%i at %d, %d: within 0.1 mm of PROJ, both ways', (code, lon, lat, x, y) => {
    const crs = getCrs(`EPSG:${code}`);
    const [fx = 0, fy = 0] = crs.forward([lon, lat]);
    expect(Math.abs(fx - x)).toBeLessThan(1e-4);
    expect(Math.abs(fy - y)).toBeLessThan(1e-4);
    const [ilon = 0, ilat = 0] = crs.inverse([x, y]);
    // 1e-9 degree: 0.1 mm
    expect(Math.abs(ilon - lon)).toBeLessThan(1e-9);
    expect(Math.abs(ilat - lat)).toBeLessThan(1e-9);
  });

  test('round trips across France and a UTM zone, to the micrometre', () => {
    const l93 = getCrs('EPSG:2154');
    const utm = getCrs('EPSG:32632');
    const lons = Array.from({ length: 11 }, (_, k) => -5 + 1.5 * k);
    const lats = Array.from({ length: 11 }, (_, k) => 41 + k);
    lons.forEach((lon) => {
      lats.forEach((lat) => {
        const [x = 0, y = 0] = l93.forward([lon, lat]);
        const [a = 0, b = 0] = l93.inverse([x, y]);
        expect(Math.hypot(a - lon, b - lat)).toBeLessThan(1e-11);
        const [u = 0, v = 0] = utm.forward([lon + 4, lat]);
        const [c = 0, d = 0] = utm.inverse([u, v]);
        expect(Math.hypot(c - lon - 4, d - lat)).toBeLessThan(1e-11);
      });
    });
  });

  test('Web Mercator in metres, longitudes and latitudes as they are', () => {
    const [x = 0, y = 0] = getCrs('EPSG:3857').forward([180, 0]);
    expect(x).toBeCloseTo(20037508.342789244, 6);
    expect(y).toBeCloseTo(0, 6);
    const [lon = 0, lat = 0] = getCrs('EPSG:3857').inverse([0, 20037508.342789244]);
    expect(lon).toBe(0);
    expect(lat).toBeCloseTo(85.0511287798, 9);
    expect(getCrs('EPSG:4326').forward([2.5, 48])).toEqual([2.5, 48]);
  });

  test('names: EPSG, URN, URL, CRS84, the IGN, the old names of Web Mercator', () => {
    expect(epsgCode('EPSG:2154')).toBe(2154);
    expect(epsgCode('urn:ogc:def:crs:EPSG::2154')).toBe(2154);
    expect(epsgCode('urn:ogc:def:crs:EPSG:6.3:3946')).toBe(3946);
    expect(epsgCode('http://www.opengis.net/def/crs/EPSG/0/32631')).toBe(32631);
    expect(epsgCode('urn:ogc:def:crs:OGC:1.3:CRS84')).toBe(4326);
    expect(epsgCode('CRS:84')).toBe(4326);
    expect(epsgCode('IGNF:LAMB93')).toBe(2154);
    expect(epsgCode('EPSG:900913')).toBe(3857);
    expect(epsgCode('LAMB93')).toBeUndefined();
    expect(isGeographic('EPSG:4326') && isGeographic('EPSG:4171') && !isGeographic('EPSG:2154')).toBe(true);
    expect(getCrs('urn:ogc:def:crs:EPSG::2154')).toBe(getCrs('EPSG:2154'));
  });

  test('an unknown CRS throws, naming the way to register it', () => {
    expect(() => getCrs('EPSG:27572')).toThrow("registerCrs('EPSG:27572', proj4('EPSG:27572'))");
    expect(() => getCrs('EPSG:32661')).toThrow();
  });
});

describe('plugged CRSs', () => {
  /** A stand-in for a proj4 converter (`proj4('EPSG:…')`): metres east and north of 0° on a sphere */
  const plate: Crs = {
    forward: ([lon, lat]) => [lon * 100000, lat * 100000],
    inverse: ([x, y]) => [x / 100000, y / 100000],
  };

  test('an object with forward and inverse is a CRS; registered, its name too', () => {
    expect(getCrs(plate)).toBe(plate);
    registerCrs('EPSG:99999', plate);
    expect(getCrs('urn:ogc:def:crs:EPSG::99999')).toBe(plate);
    registerCrs('local:plate', plate);
    expect(getCrs('LOCAL:PLATE')).toBe(plate);
  });
});

describe('reproject', () => {
  const collection = (name?: string): FeatureCollection => ({
    type: 'FeatureCollection',
    ...(name ? { crs: { type: 'name', properties: { name } } } : {}),
    features: [
      {
        type: 'Feature',
        properties: { a: 1 },
        geometry: { type: 'Point', coordinates: [652011.8817, 6860882.5655, 35] },
      },
      {
        type: 'Feature',
        properties: { b: 2 },
        geometry: {
          type: 'GeometryCollection',
          geometries: [
            {
              type: 'LineString',
              coordinates: [
                [700000, 6600000],
                [652011.8817, 6860882.5655],
              ],
            },
            {
              type: 'MultiPolygon',
              coordinates: [
                [
                  [
                    [700000, 6600000],
                    [700100, 6600000],
                    [700000, 6600100],
                    [700000, 6600000],
                  ],
                ],
              ],
            },
          ],
        },
      },
    ],
  });

  test('from the CRS its crs member names, altitudes and properties kept', () => {
    const data = collection('urn:ogc:def:crs:EPSG::2154');
    const out = reproject(data);
    expect(out.crs).toBeUndefined();
    const point = out.features[0]?.geometry as { coordinates: number[] };
    expect(point.coordinates[0]).toBeCloseTo(2.3461, 9);
    expect(point.coordinates[1]).toBeCloseTo(48.8462, 9);
    expect(point.coordinates[2]).toBe(35);
    expect(out.features[1]?.properties).toBe(data.features[1]?.properties as Record<string, unknown>);
    const both = out.features[1]?.geometry as { geometries: { coordinates: number[][] }[] } | undefined;
    const line = both?.geometries[0];
    expect(line?.coordinates[0]?.[0]).toBeCloseTo(3, 12);
    expect(line?.coordinates[0]?.[1]).toBeCloseTo(46.5, 12);
    // The input untouched
    expect((data.features[0]?.geometry as { coordinates: number[] } | undefined)?.coordinates[0]).toBe(652011.8817);
  });

  test('from a CRS given; nothing to do in longitudes and latitudes or without a CRS', () => {
    const out = reproject(collection().features, 'EPSG:2154');
    expect((out.features[0]?.geometry as { coordinates: number[] } | undefined)?.coordinates[1]).toBeCloseTo(
      48.8462,
      9,
    );
    const wgs84 = collection('urn:ogc:def:crs:OGC:1.3:CRS84');
    expect(reproject(wgs84).features).toBe(wgs84.features);
    expect(reproject(collection()).features[0]?.geometry).toEqual(collection().features[0]?.geometry ?? null);
  });

  test('the feature layers reproject a collection naming its CRS, once', () => {
    const data = collection('EPSG:2154');
    const features = featuresOf(data);
    expect((features[0]?.geometry as { coordinates: number[] } | undefined)?.coordinates[0]).toBeCloseTo(2.3461, 9);
    expect(featuresOf(data)).toBe(features);
    const plain = collection();
    expect(featuresOf(plain)).toBe(plain.features);
  });
});
