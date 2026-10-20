/**
 * PMTiles v3 archives: one file of tiles read by HTTP range requests (or from a `Blob`), as Protomaps publishes them.
 * The header and root directory in one request, the leaf directories as needed (kept in a small cache), each tile in
 * its own range; gzip through `DecompressionStream`. The same reader in the page (raster tiles) and in the basemap
 * workers (vector tiles of a style source `pmtiles://…`, as MapLibre's protocol names them).
 */
import { Lru } from './lru.ts';
import { withHeaders } from './requests.ts';

/** What a PMTiles archive's tiles are */
export type PmtilesTileType = 'unknown' | 'mvt' | 'png' | 'jpeg' | 'webp' | 'avif';

/** The header of a PMTiles archive (v3) */
export interface PmtilesHeader {
  rootDirectoryOffset: number;
  rootDirectoryLength: number;
  metadataOffset: number;
  metadataLength: number;
  leafDirectoryOffset: number;
  leafDirectoryLength: number;
  tileDataOffset: number;
  tileDataLength: number;
  /** 1 none, 2 gzip, 3 brotli, 4 zstd (only none and gzip are read) */
  internalCompression: number;
  tileCompression: number;
  tileType: PmtilesTileType;
  minZoom: number;
  maxZoom: number;
  /** `[west, south, east, north]`, degrees */
  bounds: [number, number, number, number];
  center: [lon: number, lat: number, zoom: number];
}

/** An entry of a directory: a run of tiles (`runLength` > 0) or a leaf directory (0) */
export interface PmtilesEntry {
  tileId: number;
  offset: number;
  length: number;
  runLength: number;
}

const HEADER = 127;
/** The header and the root directory lie within the first 16 KB of the file */
const FIRST = 16384;
const TYPES: PmtilesTileType[] = ['unknown', 'mvt', 'png', 'jpeg', 'webp', 'avif'];

/** Tiles before level z: (4^z - 1) / 3 */
const levelStart = (z: number) => (4 ** z - 1) / 3;

/** The tile id of z/x/y: the tiles of the lower levels, then the position along the level's Hilbert curve */
export function zxyToTileId(z: number, x: number, y: number): number {
  if (z > 26) throw new Error(`isard: PMTiles level ${z} past 26`);
  const n = 2 ** z;
  if (x < 0 || y < 0 || x >= n || y >= n) throw new Error(`isard: PMTiles tile ${z}/${x}/${y} out of its level`);
  let [px, py] = [x, y];
  let d = 0;
  for (let s = n / 2; s >= 1; s /= 2) {
    const rx = (px & s) > 0 ? 1 : 0;
    const ry = (py & s) > 0 ? 1 : 0;
    d += s * s * ((3 * rx) ^ ry);
    if (ry === 0) {
      if (rx === 1) [px, py] = [s - 1 - px, s - 1 - py];
      [px, py] = [py, px];
    }
  }
  return levelStart(z) + d;
}

/** The z/x/y of a tile id (the inverse of `zxyToTileId`) */
export function tileIdToZxy(id: number): [z: number, x: number, y: number] {
  let z = 0;
  while (levelStart(z + 1) <= id) z += 1;
  const n = 2 ** z;
  let t = id - levelStart(z);
  let [x, y] = [0, 0];
  for (let s = 1; s < n; s *= 2) {
    // Arithmetic, not bitwise: positions pass 32 bits past level 16
    const rx = Math.floor(t / 2) % 2;
    const ry = (t % 2) ^ rx;
    if (ry === 0) {
      if (rx === 1) [x, y] = [s - 1 - x, s - 1 - y];
      [x, y] = [y, x];
    }
    x += s * rx;
    y += s * ry;
    t = Math.floor(t / 4);
  }
  return [z, x, y];
}

/** A reader of unsigned varints past 32 bits (tile ids reach 2^52) */
class Varints {
  pos = 0;
  constructor(private readonly bytes: Uint8Array) {}
  next(): number {
    let value = 0;
    let scale = 1;
    for (;;) {
      const b = this.bytes[this.pos++];
      if (b === undefined) throw new Error('isard: PMTiles directory truncated');
      value += (b & 0x7f) * scale;
      if (b < 0x80) return value;
      scale *= 128;
      if (scale > 2 ** 63) throw new Error('isard: PMTiles varint too long');
    }
  }
}

/** The entries of a directory (decompressed) */
export function parseDirectory(bytes: Uint8Array): PmtilesEntry[] {
  const v = new Varints(bytes);
  const count = v.next();
  if (count > bytes.length) throw new Error('isard: PMTiles directory larger than its bytes');
  const entries: PmtilesEntry[] = [];
  let id = 0;
  for (let i = 0; i < count; i += 1) {
    id += v.next();
    entries.push({ tileId: id, offset: 0, length: 0, runLength: 1 });
  }
  for (const e of entries) e.runLength = v.next();
  for (const e of entries) e.length = v.next();
  entries.forEach((e, i) => {
    const value = v.next();
    const previous = entries[i - 1];
    // 0 after the first: right after the previous entry's bytes
    e.offset = value === 0 && previous ? previous.offset + previous.length : value - 1;
  });
  return entries;
}

/** The entry holding a tile id: its run of tiles, or the leaf directory it falls in; undefined if none */
export function findEntry(entries: readonly PmtilesEntry[], id: number): PmtilesEntry | undefined {
  let [lo, hi] = [0, entries.length - 1];
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    const e = entries[mid] as PmtilesEntry;
    if (e.tileId === id) return e;
    if (e.tileId < id) lo = mid + 1;
    else hi = mid - 1;
  }
  const e = entries[hi];
  if (!e) return undefined;
  if (e.runLength === 0 || id - e.tileId < e.runLength) return e;
  return undefined;
}

/** The header of an archive from its first bytes */
export function parseHeader(bytes: Uint8Array): PmtilesHeader {
  if (bytes.length < HEADER || String.fromCharCode(...bytes.subarray(0, 7)) !== 'PMTiles') {
    throw new Error('isard: not a PMTiles archive');
  }
  if (bytes[7] !== 3) throw new Error(`isard: PMTiles version ${bytes[7]} (only 3 is read)`);
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const u64 = (at: number) => Number(view.getBigUint64(at, true));
  const deg = (at: number) => view.getInt32(at, true) / 1e7;
  return {
    rootDirectoryOffset: u64(8),
    rootDirectoryLength: u64(16),
    metadataOffset: u64(24),
    metadataLength: u64(32),
    leafDirectoryOffset: u64(40),
    leafDirectoryLength: u64(48),
    tileDataOffset: u64(56),
    tileDataLength: u64(64),
    internalCompression: view.getUint8(97),
    tileCompression: view.getUint8(98),
    tileType: TYPES[view.getUint8(99)] ?? 'unknown',
    minZoom: view.getUint8(100),
    maxZoom: view.getUint8(101),
    bounds: [deg(102), deg(106), deg(110), deg(114)],
    center: [deg(119), deg(123), view.getUint8(118)],
  };
}

/** Bytes as they are stored, decompressed (1: none, 2: gzip; 0, unknown, is read as none) */
export async function decompress(bytes: Uint8Array, compression: number): Promise<Uint8Array> {
  if (compression === 0 || compression === 1) return bytes;
  if (compression !== 2) {
    throw new Error(`isard: PMTiles compression ${compression === 3 ? 'brotli' : 'zstd'} is not read (gzip only)`);
  }
  const stream = new Blob([bytes as Uint8Array<ArrayBuffer>]).stream().pipeThrough(new DecompressionStream('gzip'));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

/** Past this, a whole file sent for a range request (a server ignoring ranges) is an error rather than kept */
const WHOLE_FILE_MAX = 64 * 1024 * 1024;

/** A PMTiles archive at a URL (range requests) or in a `Blob` (a dropped file) */
export class PMTiles {
  private head: Promise<{ header: PmtilesHeader; root: PmtilesEntry[] }> | undefined;
  /** The leaf directories read, by offset */
  private readonly leaves = new Lru<number, Promise<PmtilesEntry[]>>(64);
  /** The whole file, when the server answered a range request with it (small files, `blob:` URLs) */
  private whole: Promise<Uint8Array> | undefined;

  constructor(readonly source: string | Blob) {}

  /** `length` bytes from `offset` */
  private async range(offset: number, length: number, signal?: AbortSignal): Promise<Uint8Array> {
    if (typeof this.source !== 'string') {
      return new Uint8Array(await this.source.slice(offset, offset + length).arrayBuffer());
    }
    if (this.whole) return (await this.whole).subarray(offset, offset + length);
    const response = await fetch(
      this.source,
      withHeaders(this.source, {
        headers: { Range: `bytes=${offset}-${offset + Math.max(length, 1) - 1}` },
        ...(signal ? { signal } : {}),
      }),
    );
    if (!response.ok) throw new Error(`${this.source}: HTTP ${response.status}`);
    if (response.status === 206) return new Uint8Array(await response.arrayBuffer());
    // The whole file: kept, when small enough, for the next ranges
    const size = Number(response.headers.get('Content-Length'));
    if (size > WHOLE_FILE_MAX) {
      await response.body?.cancel();
      throw new Error(`${this.source}: the server ignores range requests (HTTP 206 expected)`);
    }
    const whole = response.arrayBuffer().then((b) => new Uint8Array(b));
    this.whole = whole;
    whole.catch(() => {
      this.whole = undefined;
    });
    return (await whole).subarray(offset, offset + length);
  }

  private load() {
    this.head ??= (async () => {
      const first = await this.range(0, FIRST);
      const header = parseHeader(first);
      const { rootDirectoryOffset: o, rootDirectoryLength: n } = header;
      const raw = o + n <= first.length ? first.subarray(o, o + n) : await this.range(o, n);
      const root = parseDirectory(await decompress(raw, header.internalCompression));
      return { header, root };
    })();
    // A failure is not kept: asked again next time
    this.head.catch(() => {
      this.head = undefined;
    });
    return this.head;
  }

  /** The archive's header */
  async header(): Promise<PmtilesHeader> {
    return (await this.load()).header;
  }

  /** The archive's JSON metadata (`vector_layers`, `attribution`, `name`…); `{}` if it has none */
  async metadata(): Promise<Record<string, unknown>> {
    const { header } = await this.load();
    if (header.metadataLength === 0) return {};
    const raw = await this.range(header.metadataOffset, header.metadataLength);
    const text = new TextDecoder().decode(await decompress(raw, header.internalCompression));
    const value: unknown = JSON.parse(text);
    return value && typeof value === 'object' && !Array.isArray(value) ? (value as Record<string, unknown>) : {};
  }

  private leaf(header: PmtilesHeader, entry: PmtilesEntry): Promise<PmtilesEntry[]> {
    const offset = header.leafDirectoryOffset + entry.offset;
    let leaf = this.leaves.get(offset);
    if (!leaf) {
      leaf = this.range(offset, entry.length).then(async (raw) =>
        parseDirectory(await decompress(raw, header.internalCompression)),
      );
      leaf.catch(() => this.leaves.delete(offset));
      this.leaves.set(offset, leaf);
    }
    return leaf;
  }

  /** The bytes of tile z/x/y, decompressed (a vector tile ungzipped); undefined where the archive has none */
  async tile(z: number, x: number, y: number, signal?: AbortSignal): Promise<Uint8Array | undefined> {
    const { header, root } = await this.load();
    if (z < header.minZoom || z > header.maxZoom) return undefined;
    const id = zxyToTileId(z, x, y);
    let entries = root;
    // The root, then at most three levels of leaves
    for (let depth = 0; depth < 4; depth += 1) {
      const entry = findEntry(entries, id);
      if (!entry) return undefined;
      if (entry.runLength > 0) {
        const raw = await this.range(header.tileDataOffset + entry.offset, entry.length, signal);
        return decompress(raw, header.tileCompression);
      }
      entries = await this.leaf(header, entry);
    }
    throw new Error('isard: PMTiles directories nested too deep');
  }
}

/** The archives opened, by URL: one reader (and its directory cache) per archive in a page or worker */
const archives = new Lru<string, PMTiles>(16);

/** The reader of an archive URL (without `pmtiles://`), shared */
export function pmtiles(url: string): PMTiles {
  let archive = archives.get(url);
  if (!archive) {
    archive = new PMTiles(url);
    archives.set(url, archive);
  }
  return archive;
}

/** The archive URL of a `pmtiles://` URL, absolute (relative to the page), or undefined for another URL */
export function pmtilesUrl(url: string, base?: string): string | undefined {
  if (!url.startsWith('pmtiles://')) return undefined;
  const inner = url.slice('pmtiles://'.length);
  const from = base ?? (globalThis as { location?: { href: string } }).location?.href;
  try {
    return from ? new URL(inner, from).href : inner;
  } catch {
    return inner;
  }
}

/** A tile URL `pmtiles://<archive>/{z}/{x}/{y}` (with its numbers in) read from its archive: its bytes, undefined
 * where there is none; undefined too (`false`) for a URL that is not one */
export function pmtilesTile(url: string, signal?: AbortSignal): Promise<Uint8Array | undefined> | false {
  const match = /^pmtiles:\/\/(.+)\/(\d+)\/(\d+)\/(\d+)$/.exec(url);
  if (!match) return false;
  const [, archive = '', z, x, y] = match;
  return pmtiles(archive).tile(Number(z), Number(x), Number(y), signal);
}
