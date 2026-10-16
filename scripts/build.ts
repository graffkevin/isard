/**
 * Builds everything: the Rust engine to WebAssembly (cargo), its JavaScript bindings (wasm-bindgen), a size and speed
 * pass (wasm-opt), then the `@ign-junn/isard` package (packages/isard/dist: module, workers, type declarations), the
 * demo and the examples (bundled together from the package sources). `--bench` also bundles the benchmark (iTowns, three.js).
 */
import { copyFileSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
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

/** Bundles entry points with the workers next to them (they are loaded by URL, relative to the bundle). The output
 * folder is emptied first: files of an older build (renamed since) would ship. `split`: the entry points share the
 * chunks of their common code (the package), loaded once by the browser for all of them. */
async function bundle(entries: string[], outdir: string, options: { split?: boolean; empty?: boolean } = {}) {
  const { split = false, empty = true } = options;
  if (empty) rmSync(join(root, outdir), { recursive: true, force: true });
  const common = {
    target: 'browser',
    format: 'esm',
    minify: true,
    sourcemap: 'linked',
    conditions: ['isard-source'],
  } satisfies Partial<Bun.BuildConfig>;
  const workers = [join(root, pkg, 'src/decoder.worker.ts'), join(root, pkg, 'src/basemap.worker.ts')];
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
  for (const result of await Promise.all(builds)) {
    if (!result.success) throw new AggregateError(result.logs, `bundling ${entries.join(', ')} failed`);
  }
  mkdirSync(join(root, outdir), { recursive: true });
  copyFileSync(join(root, wasm), join(root, outdir, 'isard_bg.wasm'));
}

/** The package: its modules as they are (an application's bundler keeps what it uses), the engine's bindings, the
 * workers bundled (loaded by URL, they must stand alone) and the engine next to them */
async function buildPackage(): Promise<void> {
  const dist = join(root, pkg, 'dist');
  rmSync(dist, { recursive: true, force: true });
  await $`bunx tsc -p ${pkg}/tsconfig.build.json`;
  mkdirSync(join(dist, 'wasm'), { recursive: true });
  copyFileSync(join(root, bindings, 'isard.js'), join(dist, 'wasm/isard.js'));
  // Without `Symbol.dispose` (ES2025 lib), which fails the type check of applications on an older lib
  const declarations = readFileSync(join(root, bindings, 'isard.d.ts'), 'utf8');
  writeFileSync(join(dist, 'wasm/isard.d.ts'), declarations.replace(/^\s*\[Symbol\.dispose\]\(\): void;\n/gm, ''));
  // After tsc: the workers it emitted module by module (imported for their types) are replaced
  await bundle([], `${pkg}/dist`, { empty: false });
}

const bundles: Promise<void>[] = [
  buildPackage(),
  bundle(['apps/demo/src/main.ts'], 'apps/demo/dist'),
  // The examples together: each page loads its own entry, the package's code and the engine once for all of them
  bundle([...new Bun.Glob('examples/*/main.ts').scanSync(root)], 'examples/dist', { split: true }),
];
if (process.argv.includes('--bench')) {
  const pages = ['runner', 'itowns', 'isard', 'tiles-renderer'].map((page) => `apps/bench/src/${page}.ts`);
  bundles.push(bundle(pages, 'apps/bench/dist'));
}
await Promise.all(bundles);

const kb = (path: string) => Bun.file(join(root, path)).size / 1024;
const modules = [...new Bun.Glob('*.js').scanSync(join(root, pkg, 'dist'))].filter((f) => !f.endsWith('.worker.js'));
const total = modules.reduce((sum, file) => sum + kb(`${pkg}/dist/${file}`), 0);
console.log(
  `engine ${kb(wasm).toFixed(0)} KB, package ${modules.length} modules ${total.toFixed(0)} KB + worker ${kb(`${pkg}/dist/decoder.worker.js`).toFixed(0)} KB`,
);
