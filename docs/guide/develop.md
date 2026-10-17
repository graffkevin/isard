# Develop and release

## Layout

The repository holds a Rust workspace (`Cargo.toml`: the crates) and a Bun workspace (`package.json`: the package,
the demo, the benchmark, the examples, these docs), side by side at the root, with their tools: `rustfmt.toml` for
Rust, `biome.json` (format and lint) and `tsconfig.base.json` for TypeScript.

```
crates/geo/        camera and projections, WGS 84 geodesy, RAF20 geoid, tile selection, label placement, polygons
crates/tiles3d/    3D Tiles tree, traversal and decoding (meshopt, quantization, b3dm, glb)
crates/engine/     the WebAssembly core: the engine, the ground, the layers (render/), picking; built twice, with and
                   without the 3D Tiles (its `tiles3d` feature)
packages/isard/    the npm package @ign-junn/isard: GeoMap, layer handles, overlays, workers
apps/demo/         the demo; apps/bench/: isard, iTowns and 3DTilesRendererJS on the same scenario
examples/          one page per example, as an application would write it
docs/              this site (VitePress)
scripts/           build (and its size budget), dev server, release
```

`crates/geo` and `crates/tiles3d` have no WebGL and no I/O; `crates/engine` keeps its WebGL and browser calls apart
from its logic (raster cache, 3D Tiles state, styles, placement): all three are tested natively. The package's pure
modules (MVT, style expressions, colors, glyphs) are tested by `bun test`.

## Commands

Requires Rust (the version of `rust-toolchain.toml`, which rustup installs with its components and the
`wasm32-unknown-unknown` target), `wasm-bindgen-cli` at the version of the `wasm-bindgen` crate in `Cargo.lock` (the
build stops with the command to install it otherwise), and Bun.

```sh
bun install
bun run dev          # builds, then serves the demo on :5180, the benchmark on /bench/, the examples on /examples/
bun run build        # the engine, the package, the demo and the examples; build:bench adds the benchmark
bun run test         # Rust tests, then the package's (bun test)
bun run lint         # Biome, rustfmt, clippy pedantic (native and wasm), warnings as errors
bun run typecheck    # strict TypeScript
bun run docs:dev     # this site, with the examples live and the API reference generated
bun run docs:check   # the API reference: every type a public signature uses is exported
```

The checks run on every push and pull request; the site is deployed once they pass on `main`.

`bun run build` ends with the sizes of what the package ships, gzipped: the two engines (`isard_core_bg.wasm`
without 3D Tiles, `isard_bg.wasm` with them), the package's modules, the engines' bindings and the workers. Each has a
budget about 5 % above its size when it was set, in `scripts/size.ts`: over one, the build fails, and so do the
checks. A growth is then raised there deliberately, in the commit that makes the code bigger (`bun scripts/size.ts`
checks the last build again).

## Release

Set the version in `packages/isard/package.json` and in the three `crates/*/Cargo.toml`, and describe it in
`CHANGELOG.md` first.

```sh
bun run release:dry   # the release checked, lint, types, tests, build, the package packed: its file list
bun run release       # the same on a clean main, then the v<version> tag pushed: the Release workflow publishes it
```

The release stops before anything is tagged if the tree is not clean, the branch is not `main`, the changelog does
not describe the version, a crate has another version, or the version is already tagged. The Release workflow
publishes on npm with provenance through npm's trusted publishing: no token on any machine. It is set up once on
npmjs.com, in the package's settings (publisher: GitHub Actions, this repository, workflow `release.yml`).
