/**
 * Serves the demo on http://127.0.0.1:5180 (PORT to change it) (127.0.0.1: on the IGN network, localhost goes through the proxy).
 * Ctrl+C stops it.
 */
import { join, normalize } from 'node:path';

// The demo at /, the benchmark at /bench/
const apps = join(import.meta.dir, '../apps');
const server = Bun.serve({
  hostname: '127.0.0.1',
  port: Number(process.env.PORT ?? 5180),
  async fetch(request) {
    const path = normalize(new URL(request.url).pathname).replace(/^(\.\.[/\\])+/, '');
    const [root, rest] = path.startsWith('/bench/')
      ? [join(apps, 'bench'), path.slice('/bench'.length)]
      : [join(apps, 'demo'), path];
    const file = Bun.file(join(root, rest.endsWith('/') ? `${rest}index.html` : rest));
    return (await file.exists()) ? new Response(file) : new Response('Not found', { status: 404 });
  },
});
console.log(`demo: ${server.url}`);
