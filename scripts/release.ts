/**
 * Publishes `@ign-junn/isard` on npm: the release checked first (clean tree on main, version described in the
 * changelog, same version in the crates, not yet tagged), then lint, types, tests and build, the README, changelog and
 * licence of the repository copied into the package, `npm publish` with NPM_TOKEN (from `.env`, which Bun loads for
 * its own scripts), and the `v<version>` tag. npm reads the token from a temporary config that names the variable,
 * never from the command line (visible to every process): npm ignores the `.npmrc` of a workspace package, and
 * `bun publish` hangs behind the IGN proxy. `--dry-run` packs without publishing nor tagging.
 */
import { copyFileSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { $ } from 'bun';

const root = join(import.meta.dir, '..');
const pkg = join(root, 'packages/isard');
const dry = process.argv.includes('--dry-run');
$.cwd(root);

function fail(message: string): never {
  console.error(message);
  process.exit(1);
}

const { version } = JSON.parse(readFileSync(join(pkg, 'package.json'), 'utf8')) as { version: string };
if (!dry && !process.env.NPM_TOKEN) {
  fail('NPM_TOKEN is empty: put NPM_TOKEN=<npm token> in .env (read and write on the ign-junn organization)');
}
if (!readFileSync(join(root, 'CHANGELOG.md'), 'utf8').includes(`## [${version}]`)) {
  fail(`CHANGELOG.md does not describe ${version}: add its "## [${version}] - <date>" section first`);
}
for (const crate of ['engine', 'geo', 'tiles3d']) {
  const manifest = readFileSync(join(root, 'crates', crate, 'Cargo.toml'), 'utf8');
  const crateVersion = /^version = "([^"]+)"/m.exec(manifest)?.[1];
  if (crateVersion !== version) fail(`crates/${crate} is at ${crateVersion}, the package at ${version}: align them`);
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
for (const file of ['README.md', 'CHANGELOG.md', 'LICENSE']) copyFileSync(join(root, file), join(pkg, file));

const config = mkdtempSync(join(tmpdir(), 'isard-release-'));
try {
  // npm expands ${NPM_TOKEN} from its environment: the token itself is written nowhere
  writeFileSync(join(config, '.npmrc'), `//registry.npmjs.org/:_authToken=${'$'}{NPM_TOKEN}\n`);
  $.cwd(pkg);
  await $`bunx npm@10 publish --access public --userconfig=${join(config, '.npmrc')} ${dry ? '--dry-run' : []}`;
} finally {
  rmSync(config, { recursive: true, force: true });
}
if (!dry) {
  $.cwd(root);
  await $`git tag -a v${version} -m ${`@ign-junn/isard ${version}`}`;
  console.log(`published ${version} and tagged v${version}: git push origin v${version}`);
}
