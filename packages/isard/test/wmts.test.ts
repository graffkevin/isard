import { afterAll, describe, expect, mock, test } from 'bun:test';
import { planTile, resolveGrid } from '../src/grid.ts';
import { parseWmtsGrid, wmtsGrid } from '../src/wmts.ts';
import { setGlobals } from './fakes.ts';

/** Two layers of the Géoplateforme's capabilities and their sets, as served */
const CAPABILITIES = await Bun.file(new URL('fixtures/geoplateforme-wmts.xml', import.meta.url)).text();

describe('parseWmtsGrid', () => {
  test("a Lambert-93 layer: its set's matrices (resolutions from the scale denominators), its limits, its extent", () => {
    const grid = parseWmtsGrid(CAPABILITIES, 'GEOGRAPHICALGRIDSYSTEMS.PLANIGNV2.L93');
    expect(grid.crs).toBe('EPSG:2154');
    expect(grid.name).toBe('2154_10cm_6_20');
    expect(grid.bounds).toEqual([-5.14151, 41.3274, 9.55802, 51.0897]);
    expect(grid.matrices?.length).toBe(15);
    const [first] = grid.matrices ?? [];
    expect(first).toMatchObject({ id: '6', origin: [0, 12000000], tileWidth: 256, matrixWidth: 2, matrixHeight: 13 });
    expect(first?.resolution).toBeCloseTo(3276.8, 9);
    expect(first?.limits).toEqual([0, 5, 1, 7]);
    expect(grid.matrices?.at(-1)?.resolution).toBeCloseTo(0.2, 12);
    // Planned as the hand-written pyramid: Paris at zoom 12 from level 13
    const job = planTile(resolveGrid(grid), '{z}/{x}/{y}', 12, 2074, 1409, 256);
    expect(job?.sources.map((s) => s.url)).toEqual(['13/98/783', '13/99/783', '13/98/784', '13/99/784']);
  });

  test('a grid in degrees: resolutions in degrees, longitude first in CRS84 and WGS84G', () => {
    const grid = parseWmtsGrid(CAPABILITIES, 'ORTHOIMAGERY.ORTHOPHOTOS.LOW_RES.CRS84', 'WGS84G_PO_0_12');
    expect(grid.crs).toBe('IGNF:WGS84G');
    const [first] = grid.matrices ?? [];
    expect(first?.resolution).toBeCloseTo(180 / 256, 9);
    expect(first?.origin).toEqual([-180, 90]);
  });

  test("EPSG:4326's latitude first in its corners; no limits: every matrix", () => {
    const xml = `<?xml version="1.0"?>
      <Capabilities xmlns="http://www.opengis.net/wmts/1.0" xmlns:ows="http://www.opengis.net/ows/1.1"><Contents>
        <Layer><ows:Identifier>a</ows:Identifier><TileMatrixSetLink><TileMatrixSet>q</TileMatrixSet></TileMatrixSetLink></Layer>
        <TileMatrixSet><ows:Identifier>q</ows:Identifier><ows:SupportedCRS>urn:ogc:def:crs:EPSG::4326</ows:SupportedCRS>
          <TileMatrix><ows:Identifier>0</ows:Identifier><ScaleDenominator>279541132.0143588675418869</ScaleDenominator>
            <TopLeftCorner>90 -180</TopLeftCorner><TileWidth>256</TileWidth><TileHeight>256</TileHeight>
            <MatrixWidth>2</MatrixWidth><MatrixHeight>1</MatrixHeight></TileMatrix>
        </TileMatrixSet></Contents></Capabilities>`;
    const grid = parseWmtsGrid(xml, 'a');
    expect(grid.matrices?.[0]?.origin).toEqual([-180, 90]);
    expect(grid.bounds).toBeUndefined();
  });

  test('a missing layer or set throws', () => {
    expect(() => parseWmtsGrid(CAPABILITIES, 'NOPE')).toThrow('no WMTS layer NOPE');
    expect(() => parseWmtsGrid(CAPABILITIES, 'GEOGRAPHICALGRIDSYSTEMS.PLANIGNV2.L93', 'PM')).toThrow('PM');
  });
});

describe('wmtsGrid', () => {
  const restore = setGlobals({
    fetch: mock(async (url: string) =>
      url.includes('broken') ? new Response('', { status: 503 }) : new Response(CAPABILITIES),
    ),
  });
  afterAll(restore);

  test('fetches the capabilities and reads the grid', async () => {
    const grid = await wmtsGrid(
      'https://data.geopf.fr/wmts?SERVICE=WMTS&REQUEST=GetCapabilities',
      'GEOGRAPHICALGRIDSYSTEMS.PLANIGNV2.L93',
    );
    expect(grid.name).toBe('2154_10cm_6_20');
    await expect(wmtsGrid('https://broken/wmts', 'x')).rejects.toThrow('503');
  });
});
