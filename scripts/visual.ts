/**
 * Visual regression tests of the examples and the viewer: each built example (`bun run build` first) is opened in headless Chrome
 * (SwiftShader, 640×400), every network request answered from recorded fixtures (tests/visual/fixtures), and the map
 * canvas compared with tests/visual/baseline/<name>.png (pixelmatch). Diffs go to tests/visual/out. A request without
 * fixture is answered 404 and reported, without failing: how many tiles past the screen edge a map asks for depends on
 * timing (an example that starts a little earlier asks for one more), only the pixels decide.
 *
 *   bun run test:visual                 compare every example
 *   bun run test:visual basic globe     only these (an example and its views: mars, mars-olympus-mons)
 *   bun run test:visual --record        fetch the real URLs once and store them as fixtures (rewrites the set)
 *   bun run test:visual --update        rewrite the baselines
 *
 * Chrome: CHROME_PATH, else the macOS default. Requests are intercepted with the Fetch domain of the BROWSER target:
 * it sees the requests of the page's workers too (tiles, glyphs, sprites are fetched in workers), which puppeteer's
 * page.setRequestInterception does not.
 */
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join, normalize } from 'node:path';
import pixelmatch from 'pixelmatch';
import { PNG } from 'pngjs';
import puppeteer, { type CDPSession, type Protocol } from 'puppeteer-core';

const root = join(import.meta.dir, '..');
const dir = join(root, 'tests/visual');
const fixtures = join(dir, 'fixtures');
const baselines = join(dir, 'baseline');
const out = join(dir, 'out');
const manifestPath = join(fixtures, 'manifest.json');

const args = process.argv.slice(2);
const record = args.includes('--record');
const update = args.includes('--update') || record;
const only = args.filter((a) => !a.startsWith('--'));

const WIDTH = 640;
const HEIGHT = 400;
/** pixelmatch's color threshold, and the share of differing pixels that fails */
const THRESHOLD = 0.1;
const MAX_DIFF = 0.005;
/** Quiet network this long (and no frame asked for) means the map is done */
const QUIET_MS = 1500;
const TIMEOUT_MS = 90_000;

/**
 * Examples whose clock is frozen: the globe turns a few degrees a second from performance.now and the rAF timestamps
 * (examples/globe/main.ts). Both are pinned, so it does not move; it still asks for a frame every frame, so it is done
 * when the network is quiet (no frame check). The engine's own clock (fades, upload budgets) is Date.now, untouched.
 */
const FROZEN = new Set(['globe']);

/**
 * Examples opened with `?still`: their animation drawn at a fixed time and no frame asked for it (the three.js scene,
 * examples/threejs/main.ts), so they are done once idle like the others, their labels placed
 */
const STILL = new Set(['threejs']);

/**
 * Views of an example opened at another camera (the `#view` of its URL), run with it: the pitched views over the relief
 * of the Moon and Mars
 */
const VIEWS: Record<string, [example: string, view: string]> = {
  'moon-tycho': ['moon', 'tycho'],
  'mars-olympus-mons': ['mars', 'olympus-mons'],
  'mars-valles-marineris': ['mars', 'valles-marineris'],
};

/**
 * Examples opened with a query: the point cloud with a small point budget (its fixtures, LAZ chunks of the Île de la
 * Cité, stay a few MB)
 */
const QUERIES: Record<string, string> = { pointcloud: '?budget=250000' };

/** Smaller viewports where the data is heavy (the fixtures stay small): terrain and hillshade under a pitched camera */
const VIEWPORTS: Record<string, [number, number]> = {
  relief: [320, 200],
  tiles3d: [320, 200],
  models: [480, 300],
  pointcloud: [480, 300],
  'moon-tycho': [240, 150],
  'mars-olympus-mons': [240, 150],
  'mars-valles-marineris': [320, 200],
};

/**
 * Examples run only when named (`bun run test:visual tiles3d`, after `--record tiles3d`): the Lille photomesh refines
 * down to 5 cm tiles under its pitched camera whatever the viewport, ~110 b3dm (~12 MB even gzipped), too heavy for the
 * fixtures in the repository. Its fixtures and baseline are not committed. Valles Marineris likewise (~4 MB more of Mars
 * tiles): the relief of Mars is tested over Olympus Mons. The viewshed too while it is a prototype (the relief and the
 * BD TOPO buildings of Montmartre).
 */
const OPT_IN = new Set(['tiles3d', 'mars-valles-marineris', 'viewshed']);

/**
 * The viewer (apps/viewer/dist), tested as an example: a committed GeoJSON (tests/visual/data) added by its `?data=`
 * query, on no basemap (nothing to record)
 */
const VIEWER = 'viewer';
const VIEWER_URL = '/viewer/?basemap=none&data=../data/viewer.geojson';

const built = readdirSync(join(root, 'examples'), { withFileTypes: true })
  .filter((e) => e.isDirectory() && existsSync(join(root, 'examples/dist', e.name, 'main.js')))
  .map((e) => e.name);
if (existsSync(join(root, 'apps/viewer/dist/main.js'))) built.push(VIEWER);
const views = Object.entries(VIEWS)
  .filter(([, [example]]) => built.includes(example))
  .map(([name]) => name);
const examples = [...built, ...views]
  .filter((name) => {
    if (only.length === 0) return !OPT_IN.has(name);
    return only.includes(name) || (only.includes(VIEWS[name]?.[0] ?? '') && !OPT_IN.has(name));
  })
  .sort();
if (examples.length === 0) {
  console.error('no built example: bun run build');
  process.exit(1);
}

interface Fixture {
  url: string;
  /** The range asked for (point clouds), part of the fixture's key */
  range?: string;
  status: number;
  type: string;
}
const manifest: Record<string, Fixture> = existsSync(manifestPath)
  ? JSON.parse(readFileSync(manifestPath, 'utf8'))
  : {};
const used = new Set<string>();
const key = (url: string) => createHash('sha256').update(url).digest('hex').slice(0, 20);

// The examples folder, as scripts/serve.ts serves it (its own port)
const server = Bun.serve({
  hostname: '127.0.0.1',
  port: 0,
  async fetch(request) {
    const path = normalize(new URL(request.url).pathname).replace(/^(\.\.[/\\])+/, '');
    // The examples, the viewer's build and the data it is given
    const mounts: [string, string][] = [
      ['/examples/', 'examples/'],
      ['/viewer/', 'apps/viewer/dist/'],
      ['/data/', 'tests/visual/data/'],
    ];
    const mount = mounts.find(([prefix]) => path.startsWith(prefix));
    if (!mount) return new Response('Not found', { status: 404 });
    const local = mount[1] + path.slice(mount[0].length);
    const file = Bun.file(join(root, local.endsWith('/') ? `${local}index.html` : local));
    return (await file.exists()) ? new Response(file) : new Response('Not found', { status: 404 });
  },
});
const origin = `http://127.0.0.1:${server.port}`;

const browser = await puppeteer.launch({
  executablePath: process.env.CHROME_PATH ?? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
  headless: true,
  args: [
    '--use-angle=swiftshader',
    '--enable-unsafe-swiftshader',
    '--ignore-gpu-blocklist',
    '--no-sandbox',
    '--hide-scrollbars',
    '--font-render-hinting=none',
    `--window-size=${WIDTH},${HEIGHT}`,
  ],
  defaultViewport: { width: WIDTH, height: HEIGHT, deviceScaleFactor: 1 },
});

// Every request of every target (pages and their workers)
const session: CDPSession = await browser.target().createCDPSession();
let pending = 0;
let lastActivity = Date.now();
const missing = new Set<string>();
/** Bodies are stored gzipped (<hash>.gz): the WFS GeoJSON, vector tiles and elevation compress 2 to 10 times */
const bodyPath = (h: string) => join(fixtures, `${h}.gz`);
/** What the fixtures answered for the current example */
const served = { requests: 0, bytes: 0 };
session.on('Fetch.requestPaused', (event: Protocol.Fetch.RequestPausedEvent) => {
  const { requestId, request } = event;
  if (request.url.startsWith(origin) || request.url.startsWith('data:') || request.url.startsWith('blob:')) {
    session.send('Fetch.continueRequest', { requestId }).catch(() => {});
    return;
  }
  pending++;
  lastActivity = Date.now();
  answer(requestId, request)
    .catch((e) => console.warn(`  ${request.url}: ${e}`))
    .finally(() => {
      pending--;
      lastActivity = Date.now();
    });
});
await session.send('Fetch.enable', { patterns: [{ urlPattern: '*', requestStage: 'Request' }] });

/** Attempts at a URL the server turns away for too many requests (HTTP 429, the Géoplateforme past 10 a second) */
const BUSY_ATTEMPTS = 6;

async function answer(requestId: string, request: Protocol.Network.Request): Promise<void> {
  const range = request.headers.Range ?? request.headers.range;
  const h = key(range ? `${request.url}#${range}` : request.url);
  used.add(h);
  const cors = [
    { name: 'Access-Control-Allow-Origin', value: '*' },
    { name: 'Access-Control-Allow-Headers', value: '*' },
  ];
  if (request.method === 'OPTIONS') {
    await session.send('Fetch.fulfillRequest', { requestId, responseCode: 204, responseHeaders: cors });
    return;
  }
  if (record && !manifest[h]) {
    try {
      const headers: Record<string, string> = {
        'User-Agent': 'isard-visual-tests',
        ...(range ? { Range: range } : {}),
      };
      let response = await fetch(request.url, { headers });
      for (let attempt = 1; response.status === 429 && attempt < BUSY_ATTEMPTS; attempt += 1) {
        await sleep(1000 * attempt);
        response = await fetch(request.url, { headers });
      }
      const body = Buffer.from(await response.arrayBuffer());
      writeFileSync(bodyPath(h), Bun.gzipSync(body, { level: 9 }));
      manifest[h] = {
        url: request.url,
        ...(range ? { range } : {}),
        status: response.status,
        type: response.headers.get('content-type') ?? 'application/octet-stream',
      };
    } catch (e) {
      console.warn(`  ${request.url}: ${e}`);
    }
  }
  const fixture = manifest[h];
  if (!fixture || !existsSync(bodyPath(h))) {
    missing.add(request.url);
    await session.send('Fetch.fulfillRequest', { requestId, responseCode: 404, responseHeaders: cors, body: '' });
    return;
  }
  const body = Buffer.from(Bun.gunzipSync(readFileSync(bodyPath(h))));
  served.requests++;
  served.bytes += body.length;
  await session.send('Fetch.fulfillRequest', {
    requestId,
    responseCode: fixture.status,
    responseHeaders: [...cors, { name: 'Content-Type', value: fixture.type }],
    body: body.toString('base64'),
  });
}

/** Counts the frames asked for; for frozen examples, pins performance.now and the rAF timestamps */
const instrument = (frozen: boolean) => `(() => {
  const raf = window.requestAnimationFrame.bind(window);
  window.__frames = 0;
  ${frozen ? 'const t = performance.now(); performance.now = () => t;' : ''}
  window.requestAnimationFrame = (callback) => {
    window.__frames++;
    return raf((now) => callback(${frozen ? 't' : 'now'}));
  };
  // Only the map canvas is compared: HTML panels, controls and markers use the system fonts
  document.addEventListener('DOMContentLoaded', () => {
    const style = document.createElement('style');
    style.textContent = 'body * { visibility: hidden !important; } #map canvas { visibility: visible !important; }';
    document.head.append(style);
  });
})();`;

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

mkdirSync(fixtures, { recursive: true });
mkdirSync(baselines, { recursive: true });
rmSync(out, { recursive: true, force: true });
mkdirSync(out, { recursive: true });

let failed = 0;
for (const name of examples) {
  const frozen = FROZEN.has(name);
  const page = await browser.newPage();
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  const [width, height] = VIEWPORTS[name] ?? [WIDTH, HEIGHT];
  await page.setViewport({ width, height, deviceScaleFactor: 1 });
  await page.evaluateOnNewDocument(instrument(frozen));
  missing.clear();
  served.requests = 0;
  served.bytes = 0;
  const start = Date.now();
  const [example, view] = VIEWS[name] ?? [name, ''];
  const query = STILL.has(example) ? '?still' : (QUERIES[example] ?? '');
  const url = example === VIEWER ? VIEWER_URL : `/examples/${example}/${query}${view ? `#${view}` : ''}`;
  await page.goto(`${origin}${url}`, { waitUntil: 'load' });
  await page.waitForSelector('#map canvas', { timeout: TIMEOUT_MS });
  // Done: no request for QUIET_MS and, unless frozen, no frame asked for during the last second
  let frames = -1;
  let still = 0;
  while (Date.now() - start < TIMEOUT_MS) {
    await sleep(250);
    const now = (await page.evaluate('window.__frames')) as number;
    still = now === frames ? still + 250 : 0;
    frames = now;
    if (pending === 0 && Date.now() - lastActivity >= QUIET_MS && (frozen || still >= 1000)) break;
  }
  if (frozen) await sleep(1000);
  // Two more frames, the last one composited
  await page.evaluate('new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)))');
  const canvas = await page.$('#map canvas');
  const shot = Buffer.from((await canvas?.screenshot({ type: 'png' })) ?? []);
  await page.close();

  const timedOut = Date.now() - start >= TIMEOUT_MS;
  const notes = [
    `${served.requests} requests, ${(served.bytes / 1e6).toFixed(1)} MB`,
    timedOut ? 'not idle in time' : '',
    missing.size ? `${missing.size} requests without fixture (${[...missing][0]})` : '',
    errors.length ? `page errors: ${errors[0]}` : '',
  ].filter(Boolean);
  const baseline = join(baselines, `${name}.png`);
  if (update) {
    writeFileSync(baseline, shot);
    console.log(`${name}: baseline written ${notes.join('; ')}`);
    continue;
  }
  if (!existsSync(baseline)) {
    console.log(`${name}: FAIL no baseline (--update)`);
    failed++;
    continue;
  }
  const a = PNG.sync.read(readFileSync(baseline));
  const b = PNG.sync.read(shot);
  let ratio = 1;
  if (a.width === b.width && a.height === b.height) {
    const diff = new PNG({ width: a.width, height: a.height });
    const count = pixelmatch(a.data, b.data, diff.data, a.width, a.height, { threshold: THRESHOLD });
    ratio = count / (a.width * a.height);
    if (ratio > 0) writeFileSync(join(out, `${name}.diff.png`), PNG.sync.write(diff));
  }
  const ok = ratio <= MAX_DIFF;
  // Shown on the run's summary in CI: fixtures to record if it keeps coming back
  if (missing.size && process.env.GITHUB_ACTIONS) {
    console.log(`::warning title=${name}: requests without fixture::${[...missing].join(' ')}`);
  }
  if (!ok) {
    failed++;
    writeFileSync(join(out, `${name}.png`), shot);
  }
  console.log(`${name}: ${ok ? 'ok' : 'FAIL'} ${(ratio * 100).toFixed(3)} % differ ${notes.join('; ')}`);
}

// Recording rewrites the set: fixtures of no request any more are removed
if (record) {
  for (const h of Object.keys(manifest)) {
    if (only.length === 0 && !used.has(h)) {
      delete manifest[h];
      rmSync(bodyPath(h), { force: true });
    }
  }
  const sorted = Object.fromEntries(Object.entries(manifest).sort(([, x], [, y]) => x.url.localeCompare(y.url)));
  writeFileSync(manifestPath, `${JSON.stringify(sorted, null, 2)}\n`);
}

await browser.close();
server.stop(true);
if (failed) {
  console.error(`${failed} example(s) differ: see tests/visual/out`);
  process.exit(1);
}
