/**
 * Size budget of what the package ships, run at the end of `bun run build` (so by the checks too): the gzipped size
 * of the engines, of the package's modules, of the engines' bindings and of the workers, printed, and the build failed
 * if one is over its budget. `bun scripts/size.ts` checks the last build again.
 *
 * The budgets are about 5 % above the sizes measured when they were set: a growth is a decision, not an accident. To
 * raise one deliberately, set it to the new gzipped size printed by the build plus about 5 %, in the commit that makes
 * the code bigger (and say why); update the sizes quoted in the README and the guide if they change.
 */
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

/** Budgets: gzipped KB (1024 bytes, gzip level 9), set on 2026-10-17, the point clouds and PMTiles on 2026-10-19 */
const BUDGETS = {
  /** The engine maps load first, without 3D Tiles */
  // 225 until the icons of the symbol layers (their atlas, their quads beside the glyphs, the placement of a text and
  // its icon), about 1.9 KB, in both engines; 228 until the custom layers (the page's WebGL called within the frame,
  // the matrices of its local frames, the WebGL state set back after it), about 1.9 KB, in both engines; 230 until
  // the viewshed (its depth cube, the screen's depth, the pass comparing them), about 2.4 KB, in both engines; 234
  // until its several observers (an accumulation target, the pass tinting from it), about 0.4 KB, in both engines; 236
  // until the surface under a point (the solid scene's depth read back, map.pickSurface), about 0.7 KB; 238 until the
  // viewshed's cones drawn in 3D (their edges widened on screen), about 1.3 KB; 240 until the request headers by URL
  // prefix (setRequestHeaders) and the vertical surface query, about 1.2 KB; 242 until the swipe (its columns of the
  // canvas per layer) and the relief at a position, about 0.4 KB
  'isard_core_bg.wasm': 244,
  /** The engine with 3D Tiles, loaded for the first 3D Tiles layer */
  // 341 until the icons of the symbol layers, about 1.9 KB; 348 until the custom layers and the glTF models (their
  // instances drawn in one call, their file decoded and uploaded as a 3D Tiles tile, picked), about 7.5 KB; 356 until
  // the point clouds, about 40 KB: the 3D Tiles points (pnts, glTF points, their program, about 12 KB) and the COPC
  // layer (its head, hierarchy pages, octree traversal within a point budget, the grids positioning its points, its
  // requests to the workers held to a rate, Eye-Dome Lighting, picking, about 28 KB); its LAZ decoder is the point
  // cloud module below, not in the engines (the core engine does not change); 404 until the picking of 3D Tiles
  // features and their properties (EXT_mesh_features, EXT_structural_metadata, b3dm batch tables), about 12 KB
  'isard_bg.wasm': 416,
  /** The point cloud module (the LAZ decoder of COPC), loaded by the decoding workers with the first point cloud */
  'isard_points_bg.wasm': 46,
  /** The package's modules (dist/*.js but the workers), all together */
  // 61 until the equirectangular grid (the Moon), about 2 KB; 64 until the OpenFreeMap basemaps, the Terrarium decoding
  // of the worldwide relief, the credits by source, the glyphs of a style and the Géoplateforme helpers, about 5.5 KB;
  // 71 until the map split in modules (engine loading, settings, animations; Web Mercator and the LRU cache shared with
  // the worker), each gzipped on its own here, about 2 KB (nothing in an application's bundle), the basemap's labels
  // kept per tile, the relief and reprojection handed to the workers, about 3 KB (1.3 KB minified in a bundle); 79
  // until the data in other CRSs and the vector formats, about 16.5 KB here: the CRSs (Lambert-93, CC zones, UTM) and
  // the reprojection of rasters in any tile grid in place of the equirectangular one, about 8 KB (3.7 KB minified in a
  // bundle, the GeoJSON of the feature layers reprojected included); the WMTS capabilities, the XML reader, KML, GPX
  // and TopoJSON, about 8.5 KB, each a module of its own, in a bundle only when imported; 97 until the icons of the
  // symbol layers (the sprites, the icon atlas, the layout of an icon and its fit to the text, the icons of the
  // labels), about 7 KB here (3.3 KB minified in a bundle)
  // 104 until the point clouds (their handle, the CRS of a WKT, the palette of the classes), about 1.7 KB
  // 111 until PMTiles (the archive reader: header, directories, Hilbert tile ids, ranges, gzip; `addPmtiles`, the
  // plain style of vector archives, the raster loader), about 5.5 KB here (its comments included), in a bundle only
  // when used; 118 until the request headers, the swipe, the 3D panels (Panel) and the surface queries, about 1 KB
  modules: 120,
  /** The engines' JavaScript bindings (wasm-bindgen) */
  // 38 until the icon atlas, about 0.2 KB
  // 39 until the point clouds: their methods and the point cloud module's bindings, about 2.5 KB; 44 until the
  // request headers, the surface queries, the 3D Tiles features and the relief at a position, about 0.5 KB
  bindings: 46,
  // 13 until the COPC chunks (their range requests, the point cloud module's bindings), about 1.4 KB
  'decoder.worker.js': 15,
  // 8.6 until the raster layers of a style, flat extrusions, line gaps and zoom expressions of text sizes; 9.2 until
  // the PNG relief decoded and the equirectangular rasters reprojected in the workers, about 1.3 KB; 11 until the icons
  // of the labels (their properties, icons alone along lines, the layers past the source's deepest level), 0.7 KB
  // 11.8 until the tiles of PMTiles archives (`pmtiles://` sources: the reader, its directories cached), 1.6 KB
  'basemap.worker.js': 13.5,
} as const;

const root = join(import.meta.dir, '..');
const dist = join(root, 'packages/isard/dist');

function measure(files: string[]): { raw: number; gzip: number } {
  let raw = 0;
  let gzip = 0;
  for (const file of files) {
    const bytes = readFileSync(join(dist, file));
    raw += bytes.length;
    gzip += Bun.gzipSync(bytes, { level: 9 }).length;
  }
  return { raw, gzip };
}

/** Prints the sizes against their budgets; false if one is over */
export function checkSizes(): boolean {
  const modules = [...new Bun.Glob('*.js').scanSync(dist)].filter((file) => !file.endsWith('.worker.js')).sort();
  const groups: Record<keyof typeof BUDGETS, string[]> = {
    'isard_core_bg.wasm': ['isard_core_bg.wasm'],
    'isard_bg.wasm': ['isard_bg.wasm'],
    'isard_points_bg.wasm': ['isard_points_bg.wasm'],
    modules,
    bindings: ['wasm/isard_core.js', 'wasm/isard.js', 'wasm/isard_points.js'],
    'decoder.worker.js': ['decoder.worker.js'],
    'basemap.worker.js': ['basemap.worker.js'],
  };
  const kb = (bytes: number) => `${(bytes / 1024).toFixed(1)} KB`;
  let ok = true;
  console.log('size (gzipped, budget)');
  for (const [name, files] of Object.entries(groups) as [keyof typeof BUDGETS, string[]][]) {
    const { raw, gzip } = measure(files);
    const budget = BUDGETS[name] * 1024;
    const over = gzip > budget;
    if (over) ok = false;
    const label = name === 'modules' ? `${files.length} modules` : name;
    console.log(
      `  ${label.padEnd(20)} ${kb(raw).padStart(10)} raw ${kb(gzip).padStart(10)} gzip / ${kb(budget)}${over ? '  OVER BUDGET' : ''}`,
    );
  }
  if (!ok) console.error('A size is over its budget: see scripts/size.ts to raise it deliberately');
  return ok;
}

if (import.meta.main && !checkSizes()) process.exit(1);
