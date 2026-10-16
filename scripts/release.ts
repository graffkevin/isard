/**
 * Publishes `@ign-junn/isard` on npm: lint, types, tests and build first, then the README, changelog and licence of
 * the repository copied into the package, then `npm publish` with NPM_TOKEN (from `.env`, which Bun loads for its own
 * scripts), given on the command line: npm ignores the `.npmrc` of a workspace package, and `bun publish` hangs
 * behind the IGN proxy. `--dry-run` packs without publishing.
 */
import { copyFileSync } from 'node:fs';
import { join } from 'node:path';
import { $ } from 'bun';

const root = join(import.meta.dir, '..');
const pkg = join(root, 'packages/isard');
const dry = process.argv.includes('--dry-run');
$.cwd(root);

if (!dry && !process.env.NPM_TOKEN) {
  console.error('NPM_TOKEN is empty: copy .env.example to .env and paste the npm token after NPM_TOKEN=');
  process.exit(1);
}
await $`bun run lint`;
await $`bun run typecheck`;
await $`bun run test`;
await $`bun run build`;
for (const file of ['README.md', 'CHANGELOG.md', 'LICENSE']) copyFileSync(join(root, file), join(pkg, file));
$.cwd(pkg);
const token = `--//registry.npmjs.org/:_authToken=${process.env.NPM_TOKEN ?? ''}`;
await $`bunx npm@10 publish --access public --userconfig=/dev/null ${token} ${dry ? '--dry-run' : []}`;
