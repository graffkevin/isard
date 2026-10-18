/**
 * Releases `isard`. On a machine, `bun run release` checks the release (clean tree on main, version
 * described in the changelog, same version in the crates, not yet tagged), runs lint, types, tests and build, then
 * tags `v<version>` and pushes the tag: the Release workflow publishes it on npm, with provenance, through npm's
 * trusted publishing (no token on any machine). `--dry-run` checks and packs without tagging. `--publish` is the
 * workflow's step: the package built, the README, changelog and licence of the repository copied into it, published.
 */
import { copyFileSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { $ } from 'bun';

const root = join(import.meta.dir, '..');
const pkg = join(root, 'packages/isard');
const dry = process.argv.includes('--dry-run');
const publish = process.argv.includes('--publish');
$.cwd(root);

function fail(message: string): never {
  console.error(message);
  process.exit(1);
}

const { name, version } = JSON.parse(readFileSync(join(pkg, 'package.json'), 'utf8')) as {
  name: string;
  version: string;
};
if (!readFileSync(join(root, 'CHANGELOG.md'), 'utf8').includes(`## [${version}]`)) {
  fail(`CHANGELOG.md does not describe ${version}: add its "## [${version}] - <date>" section first`);
}
for (const crate of ['engine', 'geo', 'tiles3d']) {
  const manifest = readFileSync(join(root, 'crates', crate, 'Cargo.toml'), 'utf8');
  const crateVersion = /^version = "([^"]+)"/m.exec(manifest)?.[1];
  if (crateVersion !== version) fail(`crates/${crate} is at ${crateVersion}, the package at ${version}: align them`);
}

if (publish) {
  // In the workflow, on the tag's commit (the checks ran before tagging)
  const tag = process.env.GITHUB_REF_NAME;
  if (tag !== `v${version}`) fail(`tag ${tag} for a package at ${version}`);
  // A version published by hand (the first one of a package: trusted publishing needs the package to exist)
  if ((await $`npm view ${name}@${version} version`.nothrow().quiet().text()).trim() === version) {
    console.log(`${name}@${version} is on npm already: nothing to publish`);
    process.exit(0);
  }
  await $`bun run build`;
  for (const file of ['README.md', 'CHANGELOG.md', 'LICENSE']) copyFileSync(join(root, file), join(pkg, file));
  $.cwd(pkg);
  // npm ≥ 11.5: trusted publishing (OpenID Connect from the workflow), provenance attached
  await $`npx --yes npm@11 publish --access public --provenance`;
  process.exit(0);
}

if (!dry) {
  if ((await $`git status --porcelain`.text()).trim()) fail('uncommitted changes: commit them first');
  const branch = (await $`git branch --show-current`.text()).trim();
  if (branch !== 'main') fail(`on ${branch}: releases are made from main`);
  if ((await $`git tag --list v${version}`.text()).trim()) fail(`v${version} is already tagged: raise the version`);
}
await $`bun run lint`;
await $`bun run typecheck`;
await $`bun run test`;
await $`bun run build`;
if (dry) {
  for (const file of ['README.md', 'CHANGELOG.md', 'LICENSE']) copyFileSync(join(root, file), join(pkg, file));
  $.cwd(pkg);
  await $`npx --yes npm@11 pack --dry-run`;
} else {
  await $`git tag -a v${version} -m ${`isard ${version}`}`;
  await $`git push origin v${version}`;
  console.log(`tagged and pushed v${version}: the Release workflow publishes it on npm`);
}
