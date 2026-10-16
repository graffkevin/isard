/**
 * Builds everything: the Rust engine to WebAssembly (cargo), its JavaScript bindings (wasm-bindgen), a size and speed
 * pass (wasm-opt), then the `isard` package (packages/isard/dist: module, decoding worker,
 * type declarations) and the demo (apps/demo/dist, bundled from the package sources).
 */
import { copyFileSync, mkdirSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { $ } from 'bun';

const root = join(import.meta.dir, '..');
const bin = join(process.env.CARGO_HOME ?? join(homedir(), '.cargo'), 'bin');
process.env.PATH = `${bin}:${process.env.PATH ?? ''}`;
$.cwd(root);

const pkg = 'packages/isard';
const bindings = `${pkg}/src/wasm`;
const wasm = `${bindings}/isard_bg.wasm`;

await $`cargo build -p isard --target wasm32-unknown-unknown --release`;
await $`wasm-bindgen --target web --out-dir ${bindings} target/wasm32-unknown-unknown/release/isard.wasm`;
// Size and speed pass (binaryen, a dev dependency, run by Bun): about a third smaller, without the debug sections
const features = [
  '--enable-bulk-memory',
  '--enable-nontrapping-float-to-int',
  '--enable-sign-ext',
  '--enable-mutable-globals',
];
const strip = ['--strip-debug', '--strip-producers'];
await $`bun --bun ${join(root, 'node_modules/.bin/wasm-opt')} -O3 ${features} ${strip} ${wasm} -o ${wasm}`;

/** Bundles entry points with the decoding worker next to them (it is loaded by URL, relative to the bundle) */
async function bundle(entry: string, outdir: string): Promise<void> {
  const result = await Bun.build({
    entrypoints: [join(root, entry), join(root, pkg, 'src/decoder.worker.ts')],
    outdir: join(root, outdir),
    naming: '[name].[ext]',
    target: 'browser',
    format: 'esm',
    minify: true,
    sourcemap: 'linked',
    conditions: ['source'],
  });
  if (!result.success) throw new AggregateError(result.logs, `bundling ${entry} failed`);
  mkdirSync(join(root, outdir), { recursive: true });
  copyFileSync(join(root, wasm), join(root, outdir, 'isard_bg.wasm'));
}

await bundle(`${pkg}/src/index.ts`, `${pkg}/dist`);
await bundle('apps/demo/src/main.ts', 'apps/demo/dist');
for (const page of ['runner', 'itowns', 'isard', 'tiles-renderer'])
  await bundle(`apps/bench/src/${page}.ts`, 'apps/bench/dist');
await $`bunx tsc -p ${pkg}/tsconfig.build.json`;
// The declarations of the package refer to the engine's (Camera, PickHit…): shipped with them
mkdirSync(join(root, pkg, 'dist/wasm'), { recursive: true });
copyFileSync(join(root, bindings, 'isard.d.ts'), join(root, pkg, 'dist/wasm/isard.d.ts'));

const size = (path: string) => `${(Bun.file(join(root, path)).size / 1024).toFixed(0)} KB`;
console.log(
  `engine ${size(wasm)}, package ${size(`${pkg}/dist/index.js`)} + worker ${size(`${pkg}/dist/decoder.worker.js`)}`,
);
