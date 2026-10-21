/**
 * Releases `isard`: raising the version (packages/isard/package.json and the crates, a changelog section) and pushing it
 * to main is the release. Once the checks pass on main, the Release workflow runs `--publish`: if the version is not on
 * npm yet, the package is built and published with provenance through npm's trusted publishing (no token anywhere),
 * then the commit is tagged `v<version>` with a GitHub release of its changelog section. On a machine,
 * `bun run release:dry` checks a release before pushing it (version described in the changelog, same version in the
 * crates, not yet on npm; build, lint, types, tests) and packs it without publishing: its file list.
 */
import { copyFileSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { $ } from 'bun';

const root = join(import.meta.dir, '..');
const pkg = join(root, 'packages/isard');
const publish = process.argv.includes('--publish');
$.cwd(root);

const fail = (message: string): never => {
  console.error(message);
  process.exit(1);
};

const { name, version } = JSON.parse(readFileSync(join(pkg, 'package.json'), 'utf8')) as {
  name: string;
  version: string;
};
if (!readFileSync(join(root, 'CHANGELOG.md'), 'utf8').includes(`## [${version}]`)) {
  fail(`CHANGELOG.md does not describe ${version}: add its "## [${version}] - <date>" section first`);
}
for (const crate of ['engine', 'geo', 'pointcloud', 'tiles3d']) {
  const manifest = readFileSync(join(root, 'crates', crate, 'Cargo.toml'), 'utf8');
  const crateVersion = /^version = "([^"]+)"/m.exec(manifest)?.[1];
  if (crateVersion !== version) fail(`crates/${crate} is at ${crateVersion}, the package at ${version}: align them`);
}

const published = (await $`npm view ${name}@${version} version`.nothrow().quiet().text()).trim() === version;

if (publish) {
  // In the workflow, on the commit the checks passed on
  if (published) {
    console.log(`${name}@${version} is on npm already: nothing to publish`);
    process.exit(0);
  }
  await $`bun run build`;
  for (const file of ['README.md', 'CHANGELOG.md', 'LICENSE']) copyFileSync(join(root, file), join(pkg, file));
  $.cwd(pkg);
  // npm ≥ 11.5: trusted publishing (OpenID Connect from the workflow), provenance attached
  await $`npx --yes npm@11 publish --access public --provenance`;
  $.cwd(root);
  // The tag and the GitHub release, its notes the changelog section of the version
  const changelog = readFileSync(join(root, 'CHANGELOG.md'), 'utf8');
  const start = changelog.indexOf(`## [${version}]`);
  const next = changelog.indexOf('\n## [', start + 1);
  const notes = changelog.slice(changelog.indexOf('\n', start) + 1, next < 0 ? undefined : next).trim();
  const sha = process.env.RELEASE_SHA ?? (await $`git rev-parse HEAD`.text()).trim();
  await $`gh release create v${version} --target ${sha} --title ${`isard ${version}`} --notes ${notes}`;
  process.exit(0);
}

if (published) fail(`${name}@${version} is on npm already: raise the version`);
// The build first: the types need the engine's bindings it generates
await $`bun run build`;
await $`bun run lint`;
await $`bun run typecheck`;
await $`bun run test`;
for (const file of ['README.md', 'CHANGELOG.md', 'LICENSE']) copyFileSync(join(root, file), join(pkg, file));
$.cwd(pkg);
await $`npx --yes npm@11 pack --dry-run`;
console.log(`${name}@${version} is ready: push it to main, the Release workflow publishes it once the checks pass`);
