/**
 * Builds everything: the Rust engine to WebAssembly (cargo) and the point cloud module (the LAZ decoder of the
 * workers), their JavaScript bindings (wasm-bindgen), a size and speed pass (wasm-opt), then the `isard` package (packages/isard/dist: module, workers, type declarations), the
 * demo, the viewer and the examples (bundled together from the package sources). `--bench` also bundles the benchmark (iTowns, three.js).
 */
import { copyFileSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { $ } from 'bun';
import { checkSizes } from './size.ts';

const root = join(import.meta.dir, '..');
const bin = join(process.env.CARGO_HOME ?? join(homedir(), '.cargo'), 'bin');
process.env.PATH = `${bin}:${process.env.PATH ?? ''}`;
$.cwd(root);

const pkg = 'packages/isard';
const bindings = `${pkg}/src/wasm`;
const wasm = `${bindings}/isard_bg.wasm`;
const coreWasm = `${bindings}/isard_core_bg.wasm`;
const pointsWasm = `${bindings}/isard_points_bg.wasm`;

// The bindings generator must be the version of the wasm-bindgen crate (Cargo.lock), or the module does not load
const locked = /name = "wasm-bindgen"\nversion = "([^"]+)"/.exec(readFileSync(join(root, 'Cargo.lock'), 'utf8'))?.[1];
const installed = /wasm-bindgen ([\d.]+)/.exec(await $`wasm-bindgen --version`.nothrow().text())?.[1];
if (!installed || installed !== locked) {
  console.error(
    `wasm-bindgen ${installed ?? 'not found'}, the engine needs ${locked}: cargo install wasm-bindgen-cli --version ${locked} --locked`,
  );
  process.exit(1);
}

// The engine twice: without 3D Tiles (isard_core, a third smaller), which every map loads first, and with them (isard),
// loaded once a 3D Tiles layer is added, and by the decoding workers. The same output file: built and bound in turn
const target = 'target/wasm32-unknown-unknown/release/isard.wasm';
await $`cargo build -p isard --target wasm32-unknown-unknown --release --no-default-features`;
await $`wasm-bindgen --target web --out-dir ${bindings} --out-name isard_core ${target}`;
await $`cargo build -p isard --target wasm32-unknown-unknown --release`;
await $`wasm-bindgen --target web --out-dir ${bindings} ${target}`;
// The point cloud module: the LAZ decoder of COPC chunks, loaded by the decoding workers with the first point cloud
// (the engines do not hold it)
await $`cargo build -p isard-pointcloud --target wasm32-unknown-unknown --release --features module`;
await $`wasm-bindgen --target web --out-dir ${bindings} --out-name isard_points target/wasm32-unknown-unknown/release/isard_pointcloud.wasm`;
// Size and speed pass (binaryen, a dev dependency, run by Bun): about a third smaller, without the debug sections
const features = [
  '--enable-bulk-memory',
  '--enable-nontrapping-float-to-int',
  '--enable-sign-ext',
  '--enable-mutable-globals',
];
const strip = ['--strip-debug', '--strip-producers'];
const wasmOpt = join(root, 'node_modules/.bin/wasm-opt');
await Promise.all(
  [wasm, coreWasm, pointsWasm].map((file) => $`bun --bun ${wasmOpt} -O3 ${features} ${strip} ${file} -o ${file}`),
);

/** Bundles entry points with the workers next to them (they are loaded by URL, relative to the bundle). The output
 * folder is emptied first: files of an older build (renamed since) would ship. `split`: the entry points share the
 * chunks of their common code (the package), loaded once by the browser for all of them. */
const bundle = async (entries: string[], outdir: string, options: { split?: boolean; empty?: boolean } = {}) => {
  const { split = false, empty = true } = options;
  if (empty) rmSync(join(root, outdir), { recursive: true, force: true });
  const common = {
    target: 'browser',
    format: 'esm',
    minify: true,
    sourcemap: 'linked',
    conditions: ['isard-source'],
  } satisfies Partial<Bun.BuildConfig>;
  const workers = [join(root, pkg, 'src/workers/decoder.worker.ts'), join(root, pkg, 'src/workers/basemap.worker.ts')];
  // Workers on their own (a worker cannot share the page's chunks), entry points split or not
  const builds = split
    ? [
        Bun.build({ ...common, entrypoints: workers, outdir: join(root, outdir), naming: '[name].[ext]' }),
        Bun.build({
          ...common,
          entrypoints: entries.map((entry) => join(root, entry)),
          outdir: join(root, outdir),
          root: join(root, outdir, '..'),
          splitting: true,
          naming: { entry: '[dir]/[name].[ext]', chunk: '[name]-[hash].[ext]' },
        }),
      ]
    : [
        Bun.build({
          ...common,
          entrypoints: [...entries.map((entry) => join(root, entry)), ...workers],
          outdir: join(root, outdir),
          naming: '[name].[ext]',
        }),
      ];
  const failed = (await Promise.all(builds)).find((result) => !result.success);
  if (failed) throw new AggregateError(failed.logs, `bundling ${entries.join(', ')} failed`);
  mkdirSync(join(root, outdir), { recursive: true });
  copyFileSync(join(root, wasm), join(root, outdir, 'isard_bg.wasm'));
  copyFileSync(join(root, coreWasm), join(root, outdir, 'isard_core_bg.wasm'));
  copyFileSync(join(root, pointsWasm), join(root, outdir, 'isard_points_bg.wasm'));
};

/** The package: its modules as they are (an application's bundler keeps what it uses), the engine's bindings, the
 * workers bundled (loaded by URL, they must stand alone) and the engine next to them */
const buildPackage = async (): Promise<void> => {
  const dist = join(root, pkg, 'dist');
  rmSync(dist, { recursive: true, force: true });
  await $`bunx tsc -p ${pkg}/tsconfig.build.json`;
  mkdirSync(join(dist, 'wasm'), { recursive: true });
  ['isard', 'isard_core', 'isard_points'].forEach((name) => {
    copyFileSync(join(root, bindings, `${name}.js`), join(dist, `wasm/${name}.js`));
    // Without `Symbol.dispose` (ES2025 lib), which fails the type check of applications on an older lib
    const declarations = readFileSync(join(root, bindings, `${name}.d.ts`), 'utf8');
    writeFileSync(join(dist, `wasm/${name}.d.ts`), declarations.replace(/^\s*\[Symbol\.dispose\]\(\): void;\n/gm, ''));
  });
  // After tsc: the workers it emitted module by module (imported for their types) are replaced
  await bundle([], `${pkg}/dist`, { empty: false });
};

/** The viewer (apps/viewer): a folder that stands alone, its page, module, workers and engines side by side, every URL
 * relative (served under any path: the local server, the QGIS plugin, a webview); not in the npm package */
const buildViewer = async (): Promise<void> => {
  await bundle(['apps/viewer/src/main.ts'], 'apps/viewer/dist');
  copyFileSync(join(root, 'apps/viewer/index.html'), join(root, 'apps/viewer/dist/index.html'));
};

const bundles: Promise<void>[] = [
  buildPackage(),
  bundle(['apps/demo/src/main.ts'], 'apps/demo/dist'),
  buildViewer(),
  // The examples together: each page loads its own entry, the package's code and the engine once for all of them
  bundle([...new Bun.Glob('examples/*/main.{ts,tsx}').scanSync(root)], 'examples/dist', { split: true }),
];
if (process.argv.includes('--bench')) {
  const pages = ['runner', 'itowns', 'isard', 'tiles-renderer'].map((page) => `apps/bench/src/${page}.ts`);
  bundles.push(bundle(pages, 'apps/bench/dist'));
}
await Promise.all(bundles);

// What the package ships, gzipped, against its budgets (scripts/size.ts): over one, the build fails
if (!checkSizes()) process.exit(1);
