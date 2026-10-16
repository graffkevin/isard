# Develop and release

## Layout

The repository holds a Rust workspace (`Cargo.toml`: the crates) and a Bun workspace (`package.json`: the package,
the demo, the benchmark, the examples, these docs), side by side at the root, with their tools: `rustfmt.toml` for
Rust, `biome.json` (format and lint) and `tsconfig.base.json` for TypeScript.

```
crates/geo/        camera and projections, WGS 84 geodesy, RAF20 geoid, tile selection, label placement, polygons
crates/tiles3d/    3D Tiles tree, traversal and decoding (meshopt, quantization, b3dm, glb)
crates/engine/     the WebAssembly core: the engine, the ground, the layers (render/), picking
packages/isard/    the npm package @ign-junn/isard: GeoMap, layer handles, overlays, workers
apps/demo/         the demo; apps/bench/: isard, iTowns and 3DTilesRendererJS on the same scenario
examples/          one page per example, as an application would write it
docs/              this site (VitePress)
scripts/           build, dev server, release
```

`crates/geo` and `crates/tiles3d` have no WebGL and no I/O: they are tested natively.

## Commands

Requires Rust with the `wasm32-unknown-unknown` target, `wasm-bindgen-cli` at the version pinned in
`crates/engine/Cargo.toml`, and Bun.

```sh
bun install
bun run dev        # builds, then serves the demo on :5180, the benchmark on /bench/, the examples on /examples/
bun run test       # Rust tests
bun run lint       # Biome, clippy pedantic (native and wasm)
bun run typecheck  # strict TypeScript
bun run docs:dev   # this site, with the examples live
```

## Release

```sh
bun run release:dry   # lint, types, tests, build, the package packed: its file list
bun run release       # the same, then published on npm (token in .env, see .env.example)
```

Set the version in `packages/isard/package.json` and describe it in `CHANGELOG.md` first.
