/**
 * Builds the engine and the package: Rust → WebAssembly (cargo), JavaScript bindings (wasm-bindgen), optional size
 * pass (wasm-opt, when installed), then the `geo-render` package in js/dist and the demo in demo/dist.
 */
import { $ } from 'bun';
import { copyFileSync, mkdirSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';

const root = join(import.meta.dir, '..');
const bin = join(process.env.CARGO_HOME ?? join(homedir(), '.cargo'), 'bin');
process.env.PATH = `${bin}:${process.env.PATH ?? ''}`;
$.cwd(root);

await $`cargo build -p geo-render --target wasm32-unknown-unknown --release`;
await $`wasm-bindgen --target web --out-dir js/src/wasm target/wasm32-unknown-unknown/release/geo_render.wasm`;
const wasm = 'js/src/wasm/geo_render_bg.wasm';
if (Bun.which('wasm-opt')) await $`wasm-opt -O3 --enable-bulk-memory --enable-nontrapping-float-to-int ${wasm} -o ${wasm}`;

// The package and the demo, each with the decoding worker next to it (loaded by URL, relative to the bundle)
for (const [entry, outdir] of [['js/src/index.ts', 'js/dist'], ['demo/main.ts', 'demo/dist']]) {
  const result = await Bun.build({
    entrypoints: [join(root, entry), join(root, 'js/src/decoder.worker.ts')],
    outdir: join(root, outdir),
    naming: '[name].[ext]',
    target: 'browser',
    format: 'esm',
    minify: true,
    sourcemap: 'linked',
  });
  if (!result.success) throw new AggregateError(result.logs, `bundling ${entry} failed`);
  mkdirSync(join(root, outdir), { recursive: true });
  copyFileSync(join(root, wasm), join(root, outdir, 'geo_render_bg.wasm'));
}
await $`bunx tsc -p js/tsconfig.json`;

const size = (path: string) => `${(Bun.file(join(root, path)).size / 1024).toFixed(0)} KB`;
console.log(`engine ${size(wasm)}, package ${size('js/dist/index.js')} + wasm`);
