/**
 * The tile grid of a WMTS layer, read from the service's capabilities: its tile matrix set (CRS, levels, origin, tile
 * sizes), the layer's limits in it and its extent, for `addRaster({ url, grid })`.
 */
import { epsgCode, isGeographic } from '../data/crs.ts';
import { child, children, childText, parseXml } from '../data/xml.ts';
import { withHeaders } from '../engine/requests.ts';
import type { TileGrid, TileMatrix } from './grid.ts';

/** Metres per degree, as WMTS counts the scale of a grid in degrees (OGC 07-057r7, 6.1) */
const METRES_PER_DEGREE = (2 * Math.PI * 6378137) / 360;
/** The size of a pixel by which WMTS turns scale denominators into resolutions, metres */
const PIXEL = 0.00028;

const numbers = (text: string | undefined) => (text ?? '').trim().split(/\s+/).map(Number);

/**
 * The tile grid of `layer` in the capabilities document `xml`: its tile matrix set `matrixSet` (default the layer's
 * first), with the layer's tile limits and longitude-latitude extent. Throws if the layer or the set is missing
 */
export const parseWmtsGrid = (xml: string, layer: string, matrixSet?: string): TileGrid => {
  const contents = child(parseXml(xml), 'Contents');
  const entry = children(contents, 'Layer').find((l) => childText(l, 'Identifier') === layer);

  if (!entry) throw new Error(`isard: no WMTS layer ${layer}`);

  const links = children(entry, 'TileMatrixSetLink');
  const link = matrixSet === undefined ? links[0] : links.find((l) => childText(l, 'TileMatrixSet') === matrixSet);
  const name = childText(link, 'TileMatrixSet');
  const set = children(contents, 'TileMatrixSet').find((s) => childText(s, 'Identifier') === name);

  if (!name || !set) throw new Error(`isard: no tile matrix set ${matrixSet ?? ''} for the WMTS layer ${layer}`);

  const crs = childText(set, 'SupportedCRS') ?? '';
  const geographic = isGeographic(crs);
  // EPSG's geographic CRSs have their latitude first (CRS:84 and the IGN's WGS84G do not)
  const latFirst = geographic && epsgCode(crs) === 4326 && /EPSG/i.test(crs);
  const unit = geographic ? METRES_PER_DEGREE : 1;
  const limits = new Map<string, TileMatrix['limits']>();

  children(child(link, 'TileMatrixSetLimits'), 'TileMatrixLimits').forEach((l) => {
    const limit = (k: string) => Number(childText(l, k));

    limits.set(childText(l, 'TileMatrix') ?? '', [
      limit('MinTileCol'),
      limit('MinTileRow'),
      limit('MaxTileCol'),
      limit('MaxTileRow'),
    ]);
  });

  const matrices = children(set, 'TileMatrix').map((m): TileMatrix => {
    const id = childText(m, 'Identifier') ?? '';
    const [a = 0, b = 0] = numbers(childText(m, 'TopLeftCorner'));
    const limit = limits.get(id);

    return {
      id,
      resolution: (Number(childText(m, 'ScaleDenominator')) * PIXEL) / unit,
      origin: latFirst ? [b, a] : [a, b],
      tileWidth: Number(childText(m, 'TileWidth') ?? 256),
      tileHeight: Number(childText(m, 'TileHeight') ?? 256),
      matrixWidth: Number(childText(m, 'MatrixWidth') ?? Infinity),
      matrixHeight: Number(childText(m, 'MatrixHeight') ?? Infinity),
      ...(limit ? { limits: limit } : {}),
    };
  });
  // A layer's limits leave out the matrices it does not have
  const served = limits.size > 0 ? matrices.filter((m) => limits.has(String(m.id))) : matrices;
  const box = child(entry, 'WGS84BoundingBox');
  const [west = Number.NaN, south = Number.NaN] = numbers(childText(box, 'LowerCorner'));
  const [east = Number.NaN, north = Number.NaN] = numbers(childText(box, 'UpperCorner'));
  const bounds: [number, number, number, number] = [west, south, east, north];

  return {
    crs,
    matrices: served,
    name,
    ...(bounds.every(Number.isFinite) ? { bounds } : {}),
  };
};

/**
 * The tile grid of a WMTS layer, from the service's capabilities at `capabilities` (fetched with CORS), for
 * `addRaster`: `await wmtsGrid('https://…/wmts?SERVICE=WMTS&REQUEST=GetCapabilities', 'LAYER', 'MATRIXSET')`. The
 * raster's URL writes the matrix set, {z} for the matrix, {x} and {y} for the column and the row
 */
export const wmtsGrid = async (
  capabilities: string | URL,
  layer: string,
  matrixSet?: string,
  options: { signal?: AbortSignal } = {},
): Promise<TileGrid> => {
  const response = await fetch(
    capabilities,
    withHeaders(capabilities, options.signal ? { signal: options.signal } : {}),
  );

  if (!response.ok) throw new Error(`isard: ${response.status} for the WMTS capabilities`);

  return parseWmtsGrid(await response.text(), layer, matrixSet);
};
