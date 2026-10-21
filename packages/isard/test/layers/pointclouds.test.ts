/** Point clouds: the CRS of a COPC file from its WKT, the palette of its classes, the handle's style and format */
import { describe, expect, test } from 'bun:test';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { lambertConformalConic } from '../../src/data/crs.ts';
import { CLASSIFICATIONS, classPalette, pointCloudCrs, wktCrs } from '../../src/layers/pointclouds.ts';

/** The WKT of a fixture of the point cloud crate (its LAS header and VLRs) */
function wkt(file: string, start: string): string {
  const bytes = readFileSync(join(import.meta.dir, '../../../../crates/pointcloud/tests/data', file));
  const text = new TextDecoder('latin1').decode(bytes);
  const from = text.indexOf(start);
  const end = text.indexOf('\0', from);
  return text.slice(from, end < 0 ? undefined : end);
}

const PARIS = wkt('paris_header.bin', 'PROJCS');
const AUTZEN = wkt('autzen_header.bin', 'COMPD_CS');

describe('the CRS of a WKT', () => {
  test('LiDAR HD: Lambert-93 in metres, its altitudes on the geoid', () => {
    expect(wktCrs(PARIS)).toEqual({ code: 2154, verticalScale: 1 });
    const crs = pointCloudCrs(PARIS, {});
    expect(crs.geoid).toBe(true);
    expect(crs.verticalScale).toBe(1);
    // Notre-Dame de Paris
    const [lon = 0, lat = 0] = crs.inverse(Float64Array.of(652_299, 6_861_633));
    expect(lon).toBeCloseTo(2.3499, 3);
    expect(lat).toBeCloseTo(48.853, 3);
    expect(pointCloudCrs(PARIS, { geoid: false, verticalScale: 2 })).toMatchObject({ geoid: false, verticalScale: 2 });
  });

  test('a compound CRS in feet: its code projected, its heights in US survey feet', () => {
    expect(wktCrs(AUTZEN)).toEqual({ code: 2992, verticalScale: 0.304800609601219 });
    // Not built in: the page gives it
    expect(() => pointCloudCrs(AUTZEN, {})).toThrow("crs` (proj4('EPSG:2992'))");
    const metres = lambertConformalConic({ lat0: 41.75, lon0: -120.5, lat1: 43, lat2: 45.5, x0: 400000, y0: 0 });
    const feet = {
      forward: ([lon, lat]: [number, number]) => metres.forward([lon, lat]).map((v) => v / 0.3048),
      inverse: ([x, y]: [number, number]) => metres.inverse([x * 0.3048, y * 0.3048]),
    };
    const crs = pointCloudCrs(AUTZEN, { crs: feet });
    expect(crs.geoid).toBe(false);
    const [lon = 0, lat = 0] = crs.inverse(Float64Array.of(637_905, 851_210));
    // Autzen Stadium, Eugene (Oregon)
    expect(lon).toBeCloseTo(-123.07, 1);
    expect(lat).toBeCloseTo(44.06, 1);
  });

  test('no WKT: the crs option, else an error', () => {
    expect(() => pointCloudCrs(undefined, {})).toThrow('names no CRS');
    expect(pointCloudCrs(undefined, { crs: 'EPSG:3946' }).geoid).toBe(true);
    expect(pointCloudCrs(undefined, { crs: 'EPSG:32631' }).geoid).toBe(false);
    // A WKT naming Lambert-93 without its code; WKT2 ids
    expect(wktCrs('PROJCS["RGF93 / Lambert-93",GEOGCS["RGF93"]]').code).toBe(2154);
    expect(wktCrs('PROJCRS["WGS 84 / UTM 31N",BASEGEOGCRS["WGS 84",ID["EPSG",4326]],ID["EPSG",32631]]').code).toBe(
      32631,
    );
  });
});

describe('classes', () => {
  test('the palette: the defaults, the page over them, unknown classes grey', () => {
    const p = classPalette({ 2: { color: '#ff0000' }, 6: { visible: false } });
    expect([...p.subarray(8, 12)]).toEqual([255, 0, 0, 255]);
    expect(p[6 * 4 + 3]).toBe(0);
    // Noise hidden by default, ground brown
    expect(p[7 * 4 + 3]).toBe(0);
    expect(CLASSIFICATIONS[2]?.visible).toBe(true);
    expect([...p.subarray(200 * 4, 200 * 4 + 4)]).toEqual([200, 200, 200, 255]);
    expect(p.length).toBe(1024);
  });
});
