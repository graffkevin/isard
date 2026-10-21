/**
 * Copies the built examples into the documentation (docs/public/live): the site shows them live, each in its
 * frame, served next to the pages.
 */
import { cpSync, rmSync } from 'node:fs';
import { join } from 'node:path';

const root = join(import.meta.dir, '..');
const target = join(root, 'docs/public/live');
rmSync(target, { recursive: true, force: true });
cpSync(join(root, 'examples'), target, {
  recursive: true,
  // The pages, their built scripts and the shared style; not the sources nor the packages
  filter: (source) =>
    !/node_modules|\.tsx?$|package\.json|tsconfig|README/.test(source.slice(root.length)) || source.endsWith('.d.ts'),
});
console.log(`examples copied to ${target}`);
