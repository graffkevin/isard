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

/** Budgets: gzipped KB (1024 bytes, gzip level 9), set on 2026-10-17 */
const BUDGETS = {
  /** The engine maps load first, without 3D Tiles */
  'isard_core_bg.wasm': 225,
  /** The engine with 3D Tiles, loaded for the first 3D Tiles layer */
  'isard_bg.wasm': 341,
  /** The package's modules (dist/*.js but the workers), all together */
  modules: 61,
  /** The engines' JavaScript bindings (wasm-bindgen) */
  bindings: 38,
  'decoder.worker.js': 13,
  'basemap.worker.js': 8.6,
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
    modules,
    bindings: ['wasm/isard_core.js', 'wasm/isard.js'],
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
