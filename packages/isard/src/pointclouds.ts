/**
 * Point clouds: a COPC file (Cloud Optimized Point Cloud: LAZ 1.4 with an octree, read by HTTP range requests, as the
 * IGN distributes LiDAR HD) or a 3D Tiles point cloud (`.pnts`, glTF points), drawn by the engine with 3D Tiles. The
 * COPC file's CRS comes from its WKT (built-in CRSs, or registered ones: `registerCrs`) or the `crs` option; altitudes
 * in NGF-IGN69 (Lambert-93, the CC zones) are raised by the RAF20 geoid height onto the ellipsoid of the relief and
 * the 3D Tiles.
 */
import { type Crs, type CrsLike, epsgCode, getCrs } from './crs.ts';
import { type Feature, parseColor, type Ramp, rampTexels } from './features.ts';
import { Layer } from './layers.ts';
import type { LayerHost } from './types.ts';
import type {
  Engine,
  PointCloudCrs,
  PointCloudInfo,
  PointCloudPoint,
  PointCloudStats,
  Tiles3dStats,
} from './wasm/isard.js';

/** What the points are colored by */
export type PointColor = 'rgb' | 'classification' | 'elevation' | 'intensity';

/** The look of a class of points */
export interface ClassStyle {
  /** CSS color */
  color?: string;
  /** false hides the class */
  visible?: boolean;
}

export interface PointCloudStyle {
  /** By RGB (files with colors), class (the ASPRS palette, `classifications`), altitude (`elevationRamp` over
   * `elevationRange`) or intensity (grey over `intensityRange`). Default: RGB when the file has colors, else class */
  colorBy?: PointColor;
  /** CSS pixels: the size of the points, or the smallest with `sizeMode: 'adaptive'` (default 2) */
  pointSize: number;
  /** CSS pixels: the largest adaptive size (default 4) */
  maxPointSize: number;
  /** `'adaptive'` (default): the spacing of the points of the finest level drawn there, at their distance (the
   * nearer, the larger, no gap); `'fixed'`: `pointSize` everywhere */
  sizeMode: 'adaptive' | 'fixed';
  /** `'circle'` (default) or `'square'` */
  shape: 'circle' | 'square';
  /** Eye-Dome Lighting: points shaded by the depth of their neighbours on screen, their relief read without normals
   * nor colors (default false; `strength` 1, `radius` 1.4 CSS pixels) */
  edl: boolean | { strength?: number; radius?: number };
  /** Colors and visibility by class code, over the defaults (ASPRS, and IGN's LiDAR HD classes 64 to 67; the noise
   * 7 and 18 and the artefacts 65 hidden): `{ 2: { color: '#a0522d' }, 66: { visible: false } }` */
  classifications: Record<number, ClassStyle>;
  /** Altitudes (metres, the file's vertical datum) mapped onto the ramp; default the file's lowest and highest */
  elevationRange?: [min: number, max: number];
  /** Colors from the lowest altitude (0) to the highest (1) */
  elevationRamp: Ramp;
  /** Intensities mapped from black to white; default the bulk of the first node's (2nd to 98th percentile) */
  intensityRange?: [min: number, max: number];
  /** CSS pixels: a node is refined while the spacing of its points covers more on screen (default 1.5; lower is finer
   * and heavier). For 3D Tiles, their screen-space error (default 16) */
  maxScreenSpaceError: number;
  /** Points drawn at most, the nearest and the coarsest first (default 3 000 000) */
  pointBudget: number;
}

export interface PointCloudOptions extends Partial<PointCloudStyle> {
  /** A COPC file (`.copc.laz`) or a 3D Tiles `tileset.json` of points */
  url: string;
  /** Default: `'3dtiles'` for a `.json` URL, else `'copc'` */
  format?: 'copc' | '3dtiles';
  /** The CRS of a COPC file without a WKT, or whose CRS is not built in (`proj4('EPSG:2992')`) */
  crs?: CrsLike;
  /** Its altitudes are above the geoid (NGF-IGN69), raised by the RAF20 geoid height; default for Lambert-93 and the
   * CC zones */
  geoid?: boolean;
  /** Metres of a unit of z; default the vertical unit of the WKT (a US survey foot…), else that of x and y */
  verticalScale?: number;
  /** Requests a second at most: 8 by default for the Géoplateforme (data.geopf.fr allows 10), no limit elsewhere */
  requestsPerSecond?: number;
}

/** The default classes: ASPRS LAS 1.4, then IGN's LiDAR HD (64 permanent structures, 65 artefacts, 66 virtual points,
 * 67 miscellaneous buildings) */
export const CLASSIFICATIONS: Readonly<Record<number, Readonly<Required<ClassStyle>>>> = {
  0: { color: '#a6a6a6', visible: true },
  1: { color: '#c8c8c8', visible: true },
  2: { color: '#b08458', visible: true },
  3: { color: '#9ccc65', visible: true },
  4: { color: '#5aa83f', visible: true },
  5: { color: '#2e7d32', visible: true },
  6: { color: '#e57352', visible: true },
  7: { color: '#ff00ff', visible: false },
  8: { color: '#ff0000', visible: true },
  9: { color: '#3d8fd6', visible: true },
  10: { color: '#8e6bb0', visible: true },
  11: { color: '#7a7a7a', visible: true },
  12: { color: '#ffe680', visible: true },
  13: { color: '#ffff00', visible: true },
  14: { color: '#ffc800', visible: true },
  15: { color: '#c89600', visible: true },
  16: { color: '#ff9632', visible: true },
  17: { color: '#b48cc8', visible: true },
  18: { color: '#ff00ff', visible: false },
  64: { color: '#8c6e5a', visible: true },
  65: { color: '#ff00ff', visible: false },
  66: { color: '#5a6ec8', visible: true },
  67: { color: '#e6966e', visible: true },
};

/** The default ramp of the altitudes, low to high */
export const ELEVATION_RAMP: Ramp = [
  [0, '#2b83ba'],
  [0.25, '#abdda4'],
  [0.5, '#ffffbf'],
  [0.75, '#fdae61'],
  [1, '#d7191c'],
];

/** The colors of the 256 classes (RGBA, alpha 0 hides one) */
export function classPalette(overrides: Record<number, ClassStyle> = {}): Uint8Array {
  const out = new Uint8Array(256 * 4);
  for (let code = 0; code < 256; code += 1) {
    const base = CLASSIFICATIONS[code] ?? { color: '#c8c8c8', visible: true };
    const own = overrides[code] ?? {};
    const [r = 0, g = 0, b = 0] = parseColor(own.color ?? base.color);
    out.set([r * 255, g * 255, b * 255, (own.visible ?? base.visible) ? 255 : 0], code * 4);
  }
  return out;
}

/** The section of a WKT opening with one of `keywords` (`PROJCS[…]`), from its keyword to its closing bracket */
function section(wkt: string, keywords: string[]): string | undefined {
  for (const keyword of keywords) {
    const start = wkt.indexOf(`${keyword}[`);
    if (start < 0) continue;
    let depth = 0;
    for (let i = start + keyword.length; i < wkt.length; i += 1) {
      if (wkt[i] === '[') depth += 1;
      else if (wkt[i] === ']' && --depth === 0) return wkt.slice(start, i + 1);
    }
  }
  return undefined;
}

/** The EPSG code a WKT section ends with (its own `AUTHORITY`, or `ID` in WKT2), the last of its nested ones */
function lastCode(part: string): number | undefined {
  const codes = [...part.matchAll(/(?:AUTHORITY|ID)\[\s*"EPSG"\s*,\s*"?(\d+)"?\s*\]/g)];
  const last = codes.at(-1)?.[1];
  return last === undefined ? undefined : Number(last);
}

/** The factor of the last `UNIT` (`LENGTHUNIT` in WKT2) of a WKT section */
function lastUnit(part: string | undefined): number | undefined {
  const units = [...(part ?? '').matchAll(/(?:LENGTH)?UNIT\[\s*"[^"]*"\s*,\s*([\d.eE+-]+)/g)];
  const factor = Number(units.at(-1)?.[1]);
  return Number.isFinite(factor) && factor > 0 ? factor : undefined;
}

/** What a WKT says of a CRS: its EPSG code (projected first), the metres of a unit of z */
export function wktCrs(wkt: string): { code: number | undefined; verticalScale: number | undefined } {
  const projected = section(wkt, ['PROJCS', 'PROJCRS']);
  const geographic = section(wkt, ['GEOGCS', 'GEOGCRS']);
  let code = lastCode(projected ?? geographic ?? '');
  if (code === undefined && /Lambert[- ]93/i.test(wkt)) code = 2154;
  const vertical = section(wkt, ['VERT_CS', 'VERTCRS']);
  const verticalScale = lastUnit(vertical ?? projected);
  return { code, verticalScale };
}

/** Whether the altitudes of a CRS are NGF-IGN69 by default: Lambert-93 and the CC zones */
const FRENCH = (code: number | undefined) => code === 2154 || (code !== undefined && code >= 3942 && code <= 3950);

/** The CRS of a COPC file for the engine, from its WKT and the options */
export function pointCloudCrs(
  wkt: string | undefined,
  options: Pick<PointCloudOptions, 'crs' | 'geoid' | 'verticalScale'>,
): PointCloudCrs {
  const parsed = wkt ? wktCrs(wkt) : { code: undefined, verticalScale: undefined };
  let crs: Crs;
  let code = parsed.code;
  if (options.crs !== undefined) {
    crs = getCrs(options.crs);
    if (typeof options.crs === 'string') code = epsgCode(options.crs) ?? code;
  } else if (code !== undefined) {
    try {
      crs = getCrs(`EPSG:${code}`);
    } catch {
      throw new Error(`the CRS of the file, EPSG:${code}, is not built in: give it as \`crs\` (proj4('EPSG:${code}'))`);
    }
  } else {
    throw new Error('the file names no CRS: give it as `crs`');
  }
  const inverse = (xy: Float64Array) => {
    const out = new Float64Array(xy.length);
    for (let i = 0; i + 1 < xy.length; i += 2) {
      const [lon = Number.NaN, lat = Number.NaN] = crs.inverse([xy[i] as number, xy[i + 1] as number]);
      out[i] = lon;
      out[i + 1] = lat;
    }
    return out;
  };
  return {
    inverse,
    geoid: options.geoid ?? (FRENCH(code) || /IGN69|NGF/.test(wkt ?? '')),
    verticalScale: options.verticalScale ?? parsed.verticalScale ?? 1,
  };
}

/** The style numbers of the engine (`setPointCloudStyle`) */
function styleNumbers(s: PointCloudStyle, rate: number): Float64Array {
  const colors: PointColor[] = ['rgb', 'classification', 'elevation', 'intensity'];
  let edl: { strength?: number; radius?: number } | undefined;
  if (s.edl) edl = s.edl === true ? {} : s.edl;
  const [emin = Number.NaN, emax = Number.NaN] = s.elevationRange ?? [];
  const [imin = Number.NaN, imax = Number.NaN] = s.intensityRange ?? [];
  return Float64Array.of(
    s.colorBy ? colors.indexOf(s.colorBy) : -1,
    s.pointSize,
    s.maxPointSize,
    s.sizeMode === 'fixed' ? 0 : 1,
    s.shape === 'square' ? 0 : 1,
    edl ? (edl.strength ?? 1) : 0,
    edl?.radius ?? 1.4,
    emin,
    emax,
    imin,
    imax,
    s.maxScreenSpaceError,
    s.pointBudget,
    rate,
  );
}

/** A point cloud (`GeoMap.addPointCloud`): COPC or 3D Tiles points; `pickFeature` gives a COPC point's attributes */
export class PointCloudLayer extends Layer {
  readonly url: string;
  readonly format: 'copc' | '3dtiles';
  private style: PointCloudStyle;
  private readonly options: PointCloudOptions;

  /** @internal */
  constructor(map: LayerHost, options: PointCloudOptions) {
    super(map);
    // Against the page: the decoding workers fetch it, and a relative URL would be theirs
    const base = typeof document === 'undefined' ? undefined : document.baseURI;
    this.url = base ? new URL(options.url, base).href : options.url;
    this.format = options.format ?? (/\.json(?:[?#]|$)/i.test(options.url) ? '3dtiles' : 'copc');
    this.options = options;
    const tiles = this.format === '3dtiles';
    this.style = {
      ...(options.colorBy ? { colorBy: options.colorBy } : {}),
      pointSize: options.pointSize ?? 2,
      maxPointSize: options.maxPointSize ?? 4,
      sizeMode: options.sizeMode ?? 'adaptive',
      shape: options.shape ?? 'circle',
      edl: options.edl ?? false,
      classifications: options.classifications ?? {},
      ...(options.elevationRange ? { elevationRange: options.elevationRange } : {}),
      elevationRamp: options.elevationRamp ?? ELEVATION_RAMP,
      ...(options.intensityRange ? { intensityRange: options.intensityRange } : {}),
      maxScreenSpaceError: options.maxScreenSpaceError ?? (tiles ? 16 : 1.5),
      pointBudget: options.pointBudget ?? 3_000_000,
    };
  }

  /** @internal Drawn by the engine with 3D Tiles (the point cloud module loads with the first one) */
  override get needsFullEngine(): boolean {
    return true;
  }

  /** Changes the style, the rest kept */
  setStyle(style: Partial<PointCloudStyle>): void {
    this.style = { ...this.style, ...style };
    // Kept for its creation while the engine with 3D Tiles loads
    if (this.id < 0) return;
    this.apply(this.map.engine, this.id);
    this.map.requestRender();
  }

  getStyle(): Readonly<PointCloudStyle> {
    return this.style;
  }

  /** Loading counters; undefined once removed and while the engine with 3D Tiles loads */
  stats(): PointCloudStats | Tiles3dStats | undefined {
    if (this.id < 0) return undefined;
    return this.format === '3dtiles' ? this.map.engine.tiles3dStats(this.id) : this.map.engine.pointCloudStats(this.id);
  }

  /** What a COPC file says of itself (points, format, CRS, center, altitudes), once its head is read */
  info(): PointCloudInfo | undefined {
    return this.id < 0 || this.format === '3dtiles' ? undefined : this.map.engine.pointCloudInfo(this.id);
  }

  /** @internal The point picked (`pickFeature`, 'hoverfeature'): a Point at its longitude, latitude and height, its
   * attributes as properties */
  featureAt(index: number): Feature<PointCloudPoint> | undefined {
    if (this.id < 0 || this.format === '3dtiles') return undefined;
    const point = this.map.engine.pointCloudPoint(this.id, index);
    if (!point) return undefined;
    return {
      type: 'Feature',
      geometry: { type: 'Point', coordinates: [...point.lonLat, point.height] },
      properties: point,
    };
  }

  private apply(engine: Engine, id: number): void {
    const s = this.style;
    if (this.format === '3dtiles') {
      engine.setTiles3dStyle(id, s.maxScreenSpaceError, 1, s.pointSize);
      return;
    }
    const rate = this.options.requestsPerSecond ?? (/^https?:\/\/data\.geopf\.fr\//.test(this.url) ? 8 : 0);
    engine.setPointCloudStyle(
      id,
      styleNumbers(s, rate),
      classPalette(s.classifications),
      rampTexels(s.elevationRamp).rgba,
    );
  }

  /** @internal */
  protected create(engine: Engine): number {
    const id =
      this.format === '3dtiles'
        ? engine.addTiles3d(this.url, this.style.maxScreenSpaceError)
        : engine.addPointCloud(this.url, (wkt) => pointCloudCrs(wkt, this.options));
    this.apply(engine, id);
    return id;
  }
}
