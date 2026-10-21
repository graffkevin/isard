/**
 * The local viewer: serves the built viewer (apps/viewer/dist) on http://127.0.0.1:5190, its bridge, and the files it
 * is given, added as layers on startup. 127.0.0.1 only; requests for another host, and posts from another origin, are
 * refused (a page opened elsewhere in the browser cannot drive it). Ctrl+C stops it.
 *
 *   bun scripts/viewer.ts [files...] [--port 5190] [--open]      (also `bun run viewer`)
 *
 *   GET  /bridge/events        host → viewer messages (Server-Sent Events), the current state replayed first
 *   POST /bridge/message       viewer → host messages: printed as JSON lines, relayed on /bridge/host-events
 *   GET  /bridge/host-events   the viewer's messages, for other local tools (Server-Sent Events)
 *   POST /bridge/send          a host message (or an array of them) from other local tools, sent to the viewer
 *   GET  /files/<token>/<name> the files given, with HTTP ranges (COPC, PMTiles)
 *   GET  /mbtiles/<token>/{z}/{x}/{y}, /mbtiles/<token>/metadata.json   MBTiles as XYZ tiles (TMS rows flipped)
 *
 * The protocol: docs/guide/viewer.md, its types in apps/viewer/src/protocol.ts.
 */
import { Database } from 'bun:sqlite';
import { randomBytes } from 'node:crypto';
import { existsSync, statSync } from 'node:fs';
import { basename, extname, join, normalize, resolve } from 'node:path';
import { collectionBounds } from '../apps/viewer/src/bounds.ts';
import type { AddMessage, Bounds, GeoJsonCollection, HostMessage, ViewerMessage } from '../apps/viewer/src/protocol.ts';
import { parseHeader } from '../packages/isard/src/basemap/pmtiles.ts';

const root = join(import.meta.dir, '..');

/**
 * The package's readers of KML, GPX, TopoJSON and other CRSs, loaded at run time: their types reach the package's
 * DOM types, which the scripts' type check (no DOM) does not have
 */
interface Formats {
  parseKml(text: string): GeoJsonCollection;
  parseGpx(text: string): GeoJsonCollection;
  parseTopoJson(topology: unknown, name?: string): GeoJsonCollection;
  reproject(data: unknown): GeoJsonCollection;
}
let formats: Promise<Formats> | undefined;
function loadFormats(): Promise<Formats> {
  const src = join(root, 'packages/isard/src');
  formats ??= Promise.all(
    ['data/kml.ts', 'data/gpx.ts', 'data/topojson.ts', 'data/crs.ts'].map(
      (m) => import(join(src, m)) as Promise<Partial<Formats>>,
    ),
  ).then((modules) => Object.assign({}, ...modules) as Formats);
  return formats;
}
const KEEPALIVE_MS = 15_000;

export interface ViewerServerOptions {
  /** 0: any free port (default 5190) */
  port?: number;
  /** Files added as layers on startup */
  files?: string[];
  /** The viewer's folder (default apps/viewer/dist) */
  root?: string;
  /** Viewer messages printed as JSON lines (default true) */
  print?: boolean;
}

export interface ViewerServer {
  server: ReturnType<typeof Bun.serve>;
  url: string;
  /** Sends host messages to the viewer (kept in the state replayed to new viewers) */
  send(message: HostMessage | HostMessage[]): void;
  stop(): void;
}

/** The replayed state: the last basemap, terrain and camera, the layers still added, in order */
export class BridgeState {
  basemap: HostMessage | undefined;
  terrain: HostMessage | undefined;
  camera: HostMessage | undefined;
  readonly layers: AddMessage[] = [];

  apply(message: HostMessage): void {
    switch (message.type) {
      case 'basemap':
        this.basemap = message;
        break;
      case 'terrain':
        this.terrain = message;
        break;
      case 'camera':
      case 'fitBounds':
        this.camera = message;
        break;
      case 'remove': {
        const i = this.layers.findIndex((l) => l.id === message.id);
        if (i >= 0) this.layers.splice(i, 1);
        break;
      }
      case 'clear':
        this.layers.length = 0;
        break;
      case 'select':
        break;
      default: {
        if (!message.type.startsWith('add')) break;
        const add = message as AddMessage;
        const i = this.layers.findIndex((l) => l.id === add.id);
        // The same id replaces the layer at its place
        if (i >= 0) this.layers[i] = add;
        else this.layers.push(add);
      }
    }
  }

  /** What a viewer connecting now is sent */
  replay(): HostMessage[] {
    return [this.basemap, this.terrain, this.camera, ...this.layers].filter((m): m is HostMessage => m !== undefined);
  }
}

/** Clients of a Server-Sent Events stream */
class Channel {
  private readonly clients = new Set<ReadableStreamDefaultController<Uint8Array>>();
  private readonly encoder = new TextEncoder();

  /** A stream sending `first` then every message published */
  open(first: unknown[], signal: AbortSignal): Response {
    let self: ReadableStreamDefaultController<Uint8Array> | undefined;
    let timer: ReturnType<typeof setInterval> | undefined;
    const close = () => {
      clearInterval(timer);
      if (self) this.clients.delete(self);
    };
    const stream = new ReadableStream<Uint8Array>({
      start: (controller) => {
        self = controller;
        controller.enqueue(this.encoder.encode(': isard\n\n'));
        for (const m of first) controller.enqueue(this.event(m));
        this.clients.add(controller);
        timer = setInterval(() => {
          try {
            controller.enqueue(this.encoder.encode(': keepalive\n\n'));
          } catch {
            close();
          }
        }, KEEPALIVE_MS);
      },
      cancel: close,
    });
    signal.addEventListener('abort', close);
    return new Response(stream, {
      headers: { 'Content-Type': 'text/event-stream', 'Cache-Control': 'no-cache', Connection: 'keep-alive' },
    });
  }

  private event(message: unknown): Uint8Array {
    return this.encoder.encode(`data: ${JSON.stringify(message)}\n\n`);
  }

  publish(message: unknown): void {
    const bytes = this.event(message);
    for (const c of this.clients) {
      try {
        c.enqueue(bytes);
      } catch {
        this.clients.delete(c);
      }
    }
  }

  close(): void {
    for (const c of this.clients) {
      try {
        c.close();
      } catch {}
    }
    this.clients.clear();
  }
}

/** A file served with HTTP ranges: 206 for one range, 416 past its end, 200 without */
export function rangeResponse(path: string, request: Request): Response {
  const file = Bun.file(path);
  const size = file.size;
  const headers: Record<string, string> = { 'Accept-Ranges': 'bytes', 'Content-Type': file.type };
  const range = request.headers.get('Range');
  const head = request.method === 'HEAD';
  if (!range) {
    return new Response(head ? null : file, { headers: { ...headers, 'Content-Length': String(size) } });
  }
  const match = /^bytes=(\d*)-(\d*)$/.exec(range.trim());
  let start = Number.NaN;
  let end = size - 1;
  if (match && (match[1] || match[2])) {
    if (match[1] === '') {
      // The last n bytes
      start = Math.max(0, size - Number(match[2]));
    } else {
      start = Number(match[1]);
      if (match[2]) end = Math.min(Number(match[2]), size - 1);
    }
  }
  if (!(start <= end) || start >= size) {
    return new Response(null, { status: 416, headers: { ...headers, 'Content-Range': `bytes */${size}` } });
  }
  return new Response(head ? null : file.slice(start, end + 1), {
    status: 206,
    headers: {
      ...headers,
      'Content-Range': `bytes ${start}-${end}/${size}`,
      'Content-Length': String(end - start + 1),
    },
  });
}

const TILE_TYPES: Record<string, string> = {
  png: 'image/png',
  jpg: 'image/jpeg',
  jpeg: 'image/jpeg',
  webp: 'image/webp',
  pbf: 'application/x-protobuf',
  mvt: 'application/x-protobuf',
};

/** An MBTiles file: its metadata, its tiles by XYZ */
export class MbTiles {
  private readonly db: Database;
  private readonly query;
  readonly metadata: Record<string, unknown>;

  constructor(path: string) {
    this.db = new Database(path, { readonly: true });
    this.query = this.db.query<{ tile_data: Uint8Array }, [number, number, number]>(
      'SELECT tile_data FROM tiles WHERE zoom_level = ? AND tile_column = ? AND tile_row = ?',
    );
    const rows = this.db.query<{ name: string; value: string }, []>('SELECT name, value FROM metadata').all();
    const metadata: Record<string, unknown> = {};
    for (const { name, value } of rows) metadata[name] = value;
    // The vector layers in the `json` row
    if (typeof metadata.json === 'string') {
      try {
        Object.assign(metadata, JSON.parse(metadata.json));
      } catch {}
      delete metadata.json;
    }
    this.metadata = metadata;
  }

  get format(): string {
    return String(this.metadata.format ?? 'png').toLowerCase();
  }

  get vector(): boolean {
    return this.format === 'pbf' || this.format === 'mvt';
  }

  /** Tile z/x/y (XYZ: the row flipped from TMS) */
  tile(z: number, x: number, y: number): Uint8Array | undefined {
    return this.query.get(z, x, 2 ** z - 1 - y)?.tile_data ?? undefined;
  }

  response(z: number, x: number, y: number): Response {
    const data = this.tile(z, x, y);
    if (!data) return new Response(null, { status: this.vector ? 204 : 404 });
    const headers: Record<string, string> = { 'Content-Type': TILE_TYPES[this.format] ?? 'application/octet-stream' };
    // Gzipped as stored (vector tiles): the browser inflates them
    if (data[0] === 0x1f && data[1] === 0x8b) headers['Content-Encoding'] = 'gzip';
    return new Response(data, { headers });
  }

  /** `[west, south, east, north]` of its metadata, if any */
  get bounds(): Bounds | undefined {
    const b = String(this.metadata.bounds ?? '')
      .split(',')
      .map(Number);
    return b.length === 4 && b.every(Number.isFinite) ? (b as Bounds) : undefined;
  }

  close(): void {
    this.db.close();
  }
}

/** GeoJSON of any kind (a collection, a feature, a geometry) as a collection in longitudes and latitudes */
export async function featureCollection(json: unknown): Promise<GeoJsonCollection> {
  const value = json as { type?: string };
  let data: unknown = value;
  if (value?.type !== 'FeatureCollection' && value?.type !== 'Feature') {
    data = { type: 'Feature', geometry: value, properties: {} };
  }
  return (await loadFormats()).reproject(data);
}

/** A file of the startup list: where it is served, and the messages adding it */
interface Served {
  token: string;
  name: string;
  path: string;
  mbtiles?: MbTiles;
}

const union = (a: Bounds | undefined, b: Bounds | undefined): Bounds | undefined => {
  if (!a) return b;
  if (!b) return a;
  return [Math.min(a[0], b[0]), Math.min(a[1], b[1]), Math.max(a[2], b[2]), Math.max(a[3], b[3])];
};

/** The messages adding a file as a layer, and its bounds; an error for a file it cannot read */
export async function fileMessages(
  file: Served,
  id: string,
): Promise<{ messages: AddMessage[]; bounds: Bounds | undefined }> {
  const lower = file.name.toLowerCase();
  const url = `files/${file.token}/${encodeURIComponent(file.name)}`;
  const ext = extname(lower);
  const named = { id, name: file.name };
  if (ext === '.geojson' || ext === '.json' || ext === '.kml' || ext === '.gpx' || ext === '.topojson') {
    const text = await Bun.file(file.path).text();
    const { parseKml, parseGpx, parseTopoJson } = await loadFormats();
    let data: GeoJsonCollection;
    if (ext === '.kml') data = parseKml(text);
    else if (ext === '.gpx') data = parseGpx(text);
    else {
      const json = JSON.parse(text) as { type?: string; objects?: Record<string, unknown> };
      if (json.type === 'Topology') data = parseTopoJson(json, Object.keys(json.objects ?? {})[0]);
      else data = await featureCollection(json);
    }
    return { messages: [{ type: 'addGeoJSON', ...named, data }], bounds: collectionBounds(data) };
  }
  if (ext === '.laz' || ext === '.las') {
    return { messages: [{ type: 'addPointCloud', ...named, url }], bounds: undefined };
  }
  if (ext === '.pmtiles') {
    const header = parseHeader(new Uint8Array(await Bun.file(file.path).slice(0, 127).arrayBuffer()));
    const kind = header.tileType === 'mvt' ? 'vector' : 'raster';
    return { messages: [{ type: 'addPmtiles', ...named, url, kind }], bounds: header.bounds };
  }
  if (ext === '.mbtiles' && file.mbtiles) {
    const m = file.mbtiles;
    const tiles = `mbtiles/${file.token}/{z}/{x}/{y}`;
    const zooms = {
      ...(m.metadata.minzoom === undefined ? {} : { minZoom: Number(m.metadata.minzoom) }),
      ...(m.metadata.maxzoom === undefined ? {} : { maxZoom: Number(m.metadata.maxzoom) }),
      ...(typeof m.metadata.attribution === 'string' ? { attribution: m.metadata.attribution } : {}),
    };
    if (m.vector) {
      const layers = Array.isArray(m.metadata.vector_layers)
        ? (m.metadata.vector_layers as { id?: unknown }[]).map((l) => l?.id).filter((l) => typeof l === 'string')
        : [];
      return {
        messages: [{ type: 'addVectorTiles', ...named, url: tiles, ...zooms, ...(layers.length ? { layers } : {}) }],
        bounds: m.bounds,
      };
    }
    return { messages: [{ type: 'addRaster', ...named, url: tiles, ...zooms }], bounds: m.bounds };
  }
  if (ext === '.glb' || ext === '.gltf') {
    // No instance: the viewer places the model at the center of its view
    return { messages: [{ type: 'addModels', ...named, url, instances: [] }], bounds: undefined };
  }
  throw new Error(`${file.name}: not a format the viewer reads`);
}

/** Starts the server: the viewer, its bridge, the files */
export async function createViewerServer(options: ViewerServerOptions = {}): Promise<ViewerServer> {
  const folder = resolve(options.root ?? join(root, 'apps/viewer/dist'));
  if (!existsSync(join(folder, 'index.html'))) {
    console.warn(`isard viewer: ${folder}/index.html is missing (bun run build); the bridge is served anyway`);
  }
  const print = options.print ?? true;
  const state = new BridgeState();
  const toViewer = new Channel();
  const toHosts = new Channel();
  const files = new Map<string, Served>();

  const send = (message: HostMessage | HostMessage[]) => {
    for (const m of Array.isArray(message) ? message : [message]) {
      state.apply(m);
      toViewer.publish(m);
    }
  };

  let port = options.port ?? 5190;
  const allowedHosts = () => [`127.0.0.1:${port}`, `localhost:${port}`];
  const allowedOrigins = () => allowedHosts().map((h) => `http://${h}`);

  const server = Bun.serve({
    hostname: '127.0.0.1',
    port,
    idleTimeout: 0,
    async fetch(request) {
      if (!allowedHosts().includes(request.headers.get('Host') ?? '')) {
        return new Response('Forbidden host', { status: 403 });
      }
      const origin = request.headers.get('Origin');
      if (request.method === 'POST' && origin && !allowedOrigins().includes(origin)) {
        return new Response('Forbidden origin', { status: 403 });
      }
      const { pathname } = new URL(request.url);
      if (pathname === '/bridge/events') return toViewer.open(state.replay(), request.signal);
      if (pathname === '/bridge/host-events') return toHosts.open([], request.signal);
      if (pathname === '/bridge/message' && request.method === 'POST') {
        let message: ViewerMessage;
        try {
          message = (await request.json()) as ViewerMessage;
        } catch {
          return new Response('Bad JSON', { status: 400 });
        }
        if (typeof message?.type !== 'string') return new Response('No type', { status: 400 });
        if (print) console.log(JSON.stringify(message));
        toHosts.publish(message);
        return new Response(null, { status: 204 });
      }
      if (pathname === '/bridge/send' && request.method === 'POST') {
        let body: unknown;
        try {
          body = await request.json();
        } catch {
          return new Response('Bad JSON', { status: 400 });
        }
        const messages = (Array.isArray(body) ? body : [body]) as HostMessage[];
        if (!messages.every((m) => typeof m?.type === 'string')) return new Response('No type', { status: 400 });
        send(messages);
        return new Response(null, { status: 204 });
      }
      const served = /^\/files\/([^/]+)\/([^/]+)$/.exec(pathname);
      if (served) {
        const file = files.get(served[1] ?? '');
        if (!file || decodeURIComponent(served[2] ?? '') !== file.name)
          return new Response('Not found', { status: 404 });
        return rangeResponse(file.path, request);
      }
      const mb = /^\/mbtiles\/([^/]+)\/(?:metadata\.json|(\d+)\/(\d+)\/(\d+)(?:\.\w+)?)$/.exec(pathname);
      if (mb) {
        const tiles = files.get(mb[1] ?? '')?.mbtiles;
        if (!tiles) return new Response('Not found', { status: 404 });
        if (mb[2] === undefined) return Response.json(tiles.metadata);
        return tiles.response(Number(mb[2]), Number(mb[3]), Number(mb[4]));
      }
      if (request.method !== 'GET' && request.method !== 'HEAD') return new Response('Not found', { status: 404 });
      const path = normalize(decodeURIComponent(pathname)).replace(/^(\.\.[/\\])+/, '');
      const target = join(folder, path.endsWith('/') ? `${path}index.html` : path);
      if (!target.startsWith(folder)) return new Response('Not found', { status: 404 });
      const file = Bun.file(target);
      if (!(await file.exists())) return new Response('Not found', { status: 404 });
      const type = target.endsWith('.wasm') ? 'application/wasm' : file.type;
      return new Response(file, { headers: { 'Content-Type': type } });
    },
  });
  port = server.port ?? port;

  // The files given: served, then added as layers in order, the view fitted to them
  const ids = new Set<string>();
  let bounds: Bounds | undefined;
  for (const given of options.files ?? []) {
    const path = resolve(given);
    if (!existsSync(path) || !statSync(path).isFile()) {
      console.error(`isard viewer: ${given}: no such file`);
      continue;
    }
    const name = basename(path);
    const file: Served = { token: randomBytes(12).toString('hex'), name, path };
    let id = name;
    for (let n = 2; ids.has(id); n += 1) id = `${name}-${n}`;
    try {
      if (name.toLowerCase().endsWith('.mbtiles')) file.mbtiles = new MbTiles(path);
      const added = await fileMessages(file, id);
      files.set(file.token, file);
      ids.add(id);
      send(added.messages);
      bounds = union(bounds, added.bounds);
    } catch (error) {
      file.mbtiles?.close();
      console.error(`isard viewer: ${given}: ${error instanceof Error ? error.message : String(error)}`);
    }
  }
  if (bounds) send({ type: 'fitBounds', bounds, padding: 40 });

  const url = `http://127.0.0.1:${port}/`;
  return {
    server,
    url,
    send,
    stop() {
      toViewer.close();
      toHosts.close();
      server.stop(true);
      for (const f of files.values()) f.mbtiles?.close();
    },
  };
}

if (import.meta.main) {
  const args = process.argv.slice(2);
  const files: string[] = [];
  let port = 5190;
  let open = false;
  for (let i = 0; i < args.length; i += 1) {
    const arg = args[i] as string;
    if (arg === '--port') port = Number(args[++i]);
    else if (arg.startsWith('--port=')) port = Number(arg.slice('--port='.length));
    else if (arg === '--open') open = true;
    else files.push(arg);
  }
  if (!Number.isInteger(port) || port < 0 || port > 65535) {
    console.error('isard viewer: --port takes a port number');
    process.exit(1);
  }
  const viewer = await createViewerServer({ port, files });
  console.error(`isard viewer: ${viewer.url}?bridge=1`);
  if (open) {
    const command = { darwin: ['open'], win32: ['cmd', '/c', 'start', ''] }[process.platform as string] ?? ['xdg-open'];
    Bun.spawn([...command, `${viewer.url}?bridge=1`], { stdout: 'ignore', stderr: 'ignore' });
  }
}
