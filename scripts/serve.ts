/** Serves the demo on http://127.0.0.1:5180 (127.0.0.1: on the IGN network, localhost goes through the proxy) */
import { join, normalize } from 'node:path';

const root = join(import.meta.dir, '../demo');
const server = Bun.serve({
  hostname: '127.0.0.1',
  port: 5180,
  async fetch(request) {
    const path = normalize(new URL(request.url).pathname).replace(/^(\.\.[/\\])+/, '');
    const file = Bun.file(join(root, path.endsWith('/') ? `${path}index.html` : path));
    return (await file.exists()) ? new Response(file) : new Response('Not found', { status: 404 });
  },
});
console.log(`demo: ${server.url}`);
