# Contributing

Thank you for helping. Isard is a map engine in two halves, a Rust core compiled to WebAssembly and a TypeScript SDK:
the [Architecture](https://ideal-adventure-zqn6y6p.pages.github.io/architecture) page shows how they work together,
the [develop guide](https://ideal-adventure-zqn6y6p.pages.github.io/guide/develop) the layout, the commands and the
release.

## Before you start

- Open an issue first for a change of behaviour or of the API, so that it can be discussed before the code.
- Requirements: Rust (the version of `rust-toolchain.toml`), `wasm-bindgen-cli` at the version of `Cargo.lock`, Bun.
  Then `bun install` and `bun run build`.

## A change

1. A branch from `main`, then a pull request to `main`: `main` only takes commits whose checks pass.
2. Run what the checks run, from the root:

   ```sh
   bun run build && bun run lint && bun run typecheck && bun run test && bun run docs:check
   CHROME_PATH=… bun run test:visual   # the examples in a headless browser, against recorded data
   ```

   The build fails when a WebAssembly module, the package's modules or a worker grows over its size budget
   (`scripts/size.ts`): raise it deliberately, with a line saying why.
3. Code: English, the conventions of the code around (`biome.json` for TypeScript, `rustfmt.toml` and the workspace
   lints for Rust: every public item documented, every `unsafe` justified). A new feature comes with its tests, its
   docs page and, when it shows on a map, an example.
4. Commit messages say what changed, as a sentence; describe the change in `CHANGELOG.md` under `[Unreleased]`.

## Releases

Raising the version (`packages/isard/package.json`, `[workspace.package]` of `Cargo.toml`, a `CHANGELOG.md` section)
and merging it to `main` is the release: the checks then publish the package on npm, with provenance, and tag it.
