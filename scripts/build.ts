/**
 * Builds everything: the Rust engine to WebAssembly (cargo), its JavaScript bindings (wasm-bindgen), a size and speed
 * pass (wasm-opt), then the `@ign-junn/isard` package (packages/isard/dist: module, workers, type declarations), the
 * demo and the examples (bundled from the package sources). `--bench` also bundles the benchmark (iTowns, three.js).
 */
import { copyFileSync, mkdirSync, readFileSync, rmSync } from 'node:fs';
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

// The bindings generator must be the version of the wasm-bindgen crate (Cargo.lock), or the module does not load
const locked = /name = "wasm-bindgen"\nversion = "([^"]+)"/.exec(readFileSync(join(root, 'Cargo.lock'), 'utf8'))?.[1];
const installed = /wasm-bindgen ([\d.]+)/.exec(await $`wasm-bindgen --version`.nothrow().text())?.[1];
if (!installed || installed !== locked) {
  console.error(
    `wasm-bindgen ${installed ?? 'not found'}, the engine needs ${locked}: cargo install wasm-bindgen-cli --version ${locked} --locked`,
  );
  process.exit(1);
}

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

/** Bundles an entry point with the workers next to it (they are loaded by URL, relative to the bundle). The output
 * folder is emptied first: files of an older build (renamed since) would ship with the package. */
async function bundle(entries: string[], outdir: string): Promise<void> {
  rmSync(join(root, outdir), { recursive: true, force: true });
  const result = await Bun.build({
    entrypoints: [
      ...entries.map((entry) => join(root, entry)),
      join(root, pkg, 'src/decoder.worker.ts'),
      join(root, pkg, 'src/basemap.worker.ts'),
    ],
    outdir: join(root, outdir),
    naming: '[name].[ext]',
    target: 'browser',
    format: 'esm',
    minify: true,
    sourcemap: 'linked',
    conditions: ['isard-source'],
  });
  if (!result.success) throw new AggregateError(result.logs, `bundling ${entries.join(', ')} failed`);
  mkdirSync(join(root, outdir), { recursive: true });
  copyFileSync(join(root, wasm), join(root, outdir, 'isard_bg.wasm'));
}

const bundles: Promise<void>[] = [
  bundle([`${pkg}/src/index.ts`], `${pkg}/dist`),
  bundle(['apps/demo/src/main.ts'], 'apps/demo/dist'),
  // Each example on its own, with the engine and workers next to it: a folder to copy as it is
  ...[...new Bun.Glob('examples/*/main.ts').scanSync(root)].map((example) =>
    bundle([example], example.replace(/main\.ts$/, 'dist')),
  ),
];
if (process.argv.includes('--bench')) {
  const pages = ['runner', 'itowns', 'isard', 'tiles-renderer'].map((page) => `apps/bench/src/${page}.ts`);
  bundles.push(bundle(pages, 'apps/bench/dist'));
}
await Promise.all(bundles);
await $`bunx tsc -p ${pkg}/tsconfig.build.json`;
// The declarations of the package refer to the engine's (Camera, PickHit…): shipped with them
mkdirSync(join(root, pkg, 'dist/wasm'), { recursive: true });
copyFileSync(join(root, bindings, 'isard.d.ts'), join(root, pkg, 'dist/wasm/isard.d.ts'));

const size = (path: string) => `${(Bun.file(join(root, path)).size / 1024).toFixed(0)} KB`;
console.log(
  `engine ${size(wasm)}, package ${size(`${pkg}/dist/index.js`)} + worker ${size(`${pkg}/dist/decoder.worker.js`)}`,
);
