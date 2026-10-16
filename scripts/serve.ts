/**
 * Serves the demo on http://127.0.0.1:5180 (PORT to change it), the benchmark on /bench/ and the examples on
 * /examples/ (127.0.0.1: on the IGN network, localhost goes through the proxy). Ctrl+C stops it.
 */
import { join, normalize } from 'node:path';

const root = join(import.meta.dir, '..');
/** URL prefixes and the folders they serve, the demo last (everything else) */
const MOUNTS: [prefix: string, folder: string][] = [
  ['/bench', join(root, 'apps/bench')],
  ['/examples', join(root, 'examples')],
  ['', join(root, 'apps/demo')],
];

const server = Bun.serve({
  hostname: '127.0.0.1',
  port: Number(process.env.PORT ?? 5180),
  async fetch(request) {
    const path = normalize(new URL(request.url).pathname).replace(/^(\.\.[/\\])+/, '');
    const [prefix, folder] = MOUNTS.find(([p]) => path === p || path.startsWith(`${p}/`)) ?? ['', root];
    const rest = path.slice(prefix.length) || '/';
    const file = Bun.file(join(folder, rest.endsWith('/') ? `${rest}index.html` : rest));
    return (await file.exists()) ? new Response(file) : new Response('Not found', { status: 404 });
  },
});
console.log(`demo: ${server.url}, examples: ${server.url}examples/`);
