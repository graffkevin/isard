/**
 * The Vite plugin of isard: its engines and workers served from one folder, nothing to configure.
 *
 * ```js
 * // vite.config.js
 * import isard from 'isard/vite';
 * export default { plugins: [isard()] };
 * ```
 *
 * In development the package is left out of Vite's dependency pre-bundling, so that its files are found next to it.
 * In a build its five files (the engines `isard_bg.wasm`, `isard_core_bg.wasm`, `isard_points_bg.wasm`, the workers
 * `decoder.worker.js`, `basemap.worker.js`) are copied to `<outDir>/<folder>/`, and the page tells the maps where they
 * are (`globalThis.isardAssetsUrl`, read as `GeoMapOptions.assetsUrl`).
 */
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const FILES = ['isard_bg.wasm', 'isard_core_bg.wasm', 'isard_points_bg.wasm', 'decoder.worker.js', 'basemap.worker.js'];

/** @param {{ folder?: string }} [options] */
export default function isard(options = {}) {
  const folder = (options.folder ?? 'isard').replace(/^\/+|\/+$/g, '');
  let base = '/';
  let building = false;
  return {
    name: 'isard',
    enforce: 'pre',
    config() {
      return { optimizeDeps: { exclude: ['isard'] } };
    },
    configResolved(config) {
      // The development server serves isard's files from where the package is, linked outside the project too (a
      // workspace, `npm link`): its folder allowed besides the others
      const allow = config.server?.fs?.allow;
      if (Array.isArray(allow)) allow.push(fileURLToPath(new URL('.', import.meta.url)));
      base = config.base.endsWith('/') ? config.base : `${config.base}/`;
      building = config.command === 'build';
    },
    // In a build, the files are found in their folder: their URLs next to the modules are not to be bundled (Vite
    // would copy the engines a second time)
    transform(code, id) {
      if (!building || !/[\\/]isard[\\/]/.test(id) || !code.includes('import.meta.url')) return null;
      const next = code.replace(
        /new URL\((["'`])(?:\.{1,2}\/)?([\w.-]+\.(?:wasm|js))\1,\s*import\.meta\.url\)/g,
        (_, quote, file) => `new URL(${quote}${file}${quote}, globalThis.isardAssetsUrl ?? location.href)`,
      );
      return next === code ? null : { code: next, map: null };
    },
    generateBundle() {
      for (const file of FILES) {
        const source = readFileSync(new URL(`./dist/${file}`, import.meta.url));
        this.emitFile({ type: 'asset', fileName: `${folder}/${file}`, source });
      }
    },
    transformIndexHtml() {
      if (!building) return [];
      // A relative base (`./`) resolves against the page
      const url = `${base}${folder}/`;
      return [
        { tag: 'script', children: `globalThis.isardAssetsUrl=${JSON.stringify(url)};`, injectTo: 'head-prepend' },
      ];
    },
  };
}
