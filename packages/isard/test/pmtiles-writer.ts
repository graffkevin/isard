/**
 * A small PMTiles v3 writer for the tests (and the fixture test/fixtures/tiny.pmtiles): tiles in, an archive out, with
 * runs of identical tiles, gzip, and leaf directories on demand. `bun packages/isard/test/pmtiles-writer.ts` writes the
 * fixture again: vector tiles of levels 0 to 3 over Paris, two layers (places with names, a road).
 */
import { writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { zxyToTileId } from '../src/pmtiles.ts';

export function varint(value: number): number[] {
  let v = value;
  const out: number[] = [];
  do {
    const b = v % 128;
    v = Math.floor(v / 128);
    out.push(v > 0 ? b | 0x80 : b);
  } while (v > 0);
  return out;
}

export interface WrittenTile {
  z: number;
  x: number;
  y: number;
  data: Uint8Array;
}

interface Entry {
  tileId: number;
  offset: number;
  length: number;
  runLength: number;
}

function directory(entries: Entry[]): number[] {
  const out = [...varint(entries.length)];
  let last = 0;
  for (const e of entries) {
    out.push(...varint(e.tileId - last));
    last = e.tileId;
  }
  for (const e of entries) out.push(...varint(e.runLength));
  for (const e of entries) out.push(...varint(e.length));
  entries.forEach((e, i) => {
    const previous = entries[i - 1];
    out.push(...varint(previous && e.offset === previous.offset + previous.length ? 0 : e.offset + 1));
  });
  return out;
}

const TYPES = { unknown: 0, mvt: 1, png: 2, jpeg: 3, webp: 4, avif: 5 } as const;

/** An archive of `tiles`; `gzip`: directories, metadata and tiles gzipped; `leafSize`: entries per leaf directory
 * (none by default) */
export function writePmtiles(
  tiles: WrittenTile[],
  options: {
    tileType?: keyof typeof TYPES;
    gzip?: boolean;
    leafSize?: number;
    metadata?: Record<string, unknown>;
    bounds?: [number, number, number, number];
  } = {},
): Uint8Array<ArrayBuffer> {
  const { tileType = 'mvt', gzip = false, leafSize, metadata = {}, bounds = [-180, -85, 180, 85] } = options;
  const pack = (bytes: Uint8Array) => (gzip ? Bun.gzipSync(bytes as Uint8Array<ArrayBuffer>) : bytes);
  const sorted = tiles
    .map((t) => ({ id: zxyToTileId(t.z, t.x, t.y), data: pack(t.data), z: t.z }))
    .sort((a, b) => a.id - b.id);
  // Tile data, identical consecutive tiles as one run
  const entries: Entry[] = [];
  const chunks: Uint8Array[] = [];
  let offset = 0;
  for (const t of sorted) {
    const last = entries[entries.length - 1];
    const lastData = chunks[chunks.length - 1];
    if (
      last &&
      lastData &&
      t.id === last.tileId + last.runLength &&
      Buffer.compare(Buffer.from(lastData), Buffer.from(t.data)) === 0
    ) {
      last.runLength += 1;
      continue;
    }
    entries.push({ tileId: t.id, offset, length: t.data.length, runLength: 1 });
    chunks.push(t.data);
    offset += t.data.length;
  }
  let root: number[];
  const leaves: number[] = [];
  if (leafSize) {
    const rootEntries: Entry[] = [];
    for (let i = 0; i < entries.length; i += leafSize) {
      const part = entries.slice(i, i + leafSize);
      const bytes = pack(new Uint8Array(directory(part)));
      rootEntries.push({ tileId: part[0]?.tileId ?? 0, offset: leaves.length, length: bytes.length, runLength: 0 });
      leaves.push(...bytes);
    }
    root = [...pack(new Uint8Array(directory(rootEntries)))];
  } else {
    root = [...pack(new Uint8Array(directory(entries)))];
  }
  const meta = pack(new TextEncoder().encode(JSON.stringify(metadata)));
  const header = new Uint8Array(127);
  const view = new DataView(header.buffer);
  header.set(new TextEncoder().encode('PMTiles'), 0);
  header[7] = 3;
  const rootOffset = 127;
  const metaOffset = rootOffset + root.length;
  const leafOffset = metaOffset + meta.length;
  const dataOffset = leafOffset + leaves.length;
  const u64 = (at: number, v: number) => view.setBigUint64(at, BigInt(v), true);
  u64(8, rootOffset);
  u64(16, root.length);
  u64(24, metaOffset);
  u64(32, meta.length);
  u64(40, leafOffset);
  u64(48, leaves.length);
  u64(56, dataOffset);
  u64(64, offset);
  u64(72, sorted.length);
  u64(80, entries.length);
  u64(88, entries.length);
  header[96] = 1;
  header[97] = gzip ? 2 : 1;
  header[98] = gzip ? 2 : 1;
  header[99] = TYPES[tileType];
  const zooms = sorted.map((t) => t.z);
  header[100] = Math.min(...zooms);
  header[101] = Math.max(...zooms);
  bounds.forEach((d, i) => {
    view.setInt32(102 + i * 4, Math.round(d * 1e7), true);
  });
  header[118] = Math.min(...zooms);
  view.setInt32(119, Math.round(((bounds[0] + bounds[2]) / 2) * 1e7), true);
  view.setInt32(123, Math.round(((bounds[1] + bounds[3]) / 2) * 1e7), true);
  const out = new Uint8Array(dataOffset + offset);
  out.set(header, 0);
  out.set(root, rootOffset);
  out.set(meta, metaOffset);
  out.set(leaves, leafOffset);
  let at = dataOffset;
  for (const c of chunks) {
    out.set(c, at);
    at += c.length;
  }
  return out;
}

// ── A vector tile writer (protobuf), for the fixture ──

const zigzag = (n: number) => (n < 0 ? -2 * n - 1 : 2 * n);
const tag = (field: number, wire: number) => varint((field << 3) | wire);
const bytes = (field: number, data: number[]) => [...tag(field, 2), ...varint(data.length), ...data];
const string = (field: number, s: string) => bytes(field, [...new TextEncoder().encode(s)]);
const uint = (field: number, v: number) => [...tag(field, 0), ...varint(v)];
const command = (id: number, count: number) => varint((count << 3) | id);

/** A layer of features: points (type 1) or lines (type 2) through tile units, with an optional `name` */
export function mvtLayer(name: string, features: { type: 1 | 2; points: [number, number][]; text?: string }[]) {
  const keys = features.some((f) => f.text) ? string(3, 'name') : [];
  const values: number[] = [];
  let next = 0;
  const encoded = features.map(({ type, points, text }) => {
    const geometry: number[] = [];
    let [x, y] = [0, 0];
    points.forEach(([px, py], i) => {
      if (type === 1 || i === 0) geometry.push(...command(1, 1));
      else if (i === 1) geometry.push(...command(2, points.length - 1));
      geometry.push(...varint(zigzag(px - x)), ...varint(zigzag(py - y)));
      [x, y] = [px, py];
    });
    let tags: number[] = [];
    if (text) {
      tags = bytes(2, [0, next++]);
      values.push(...bytes(4, string(1, text)));
    }
    return bytes(2, [...tags, ...uint(3, type), ...bytes(4, geometry)]);
  });
  return bytes(3, [...string(1, name), ...encoded.flat(), ...keys, ...values, ...uint(5, 4096), ...uint(15, 2)]);
}

const PLACES: [string, number, number][] = [
  ['Notre-Dame', 2.3499, 48.853],
  ['Tour Eiffel', 2.2945, 48.8584],
  ['Vincennes', 2.4358, 48.8427],
];

/** The fixture's tiles: levels 0 to 3, the tile over Paris at each level */
function fixtureTiles(): WrittenTile[] {
  const tiles: WrittenTile[] = [];
  for (let z = 0; z <= 3; z += 1) {
    const n = 2 ** z;
    const tx = (lon: number) => ((lon + 180) / 360) * n;
    const ty = (lat: number) => ((1 - Math.log(Math.tan(Math.PI / 4 + (lat * Math.PI) / 360)) / Math.PI) / 2) * n;
    const [x, y] = [Math.floor(tx(2.35)), Math.floor(ty(48.85))];
    const local = (lon: number, lat: number): [number, number] => [
      Math.round((tx(lon) - x) * 4096),
      Math.round((ty(lat) - y) * 4096),
    ];
    const places = mvtLayer(
      'places',
      PLACES.map(([text, lon, lat]) => ({ type: 1 as const, points: [local(lon, lat)], text })),
    );
    const roads = mvtLayer('roads', [{ type: 2, points: PLACES.map(([, lon, lat]) => local(lon, lat)) }]);
    tiles.push({ z, x, y, data: new Uint8Array([...places, ...roads]) });
  }
  return tiles;
}

if (import.meta.main) {
  const archive = writePmtiles(fixtureTiles(), {
    gzip: true,
    leafSize: 2,
    bounds: [2.2, 48.8, 2.5, 48.9],
    metadata: {
      name: 'tiny',
      attribution: '<a href="https://example.com">isard tests</a>',
      vector_layers: [{ id: 'places' }, { id: 'roads' }],
    },
  });
  const path = join(import.meta.dir, 'fixtures/tiny.pmtiles');
  writeFileSync(path, archive);
  console.log(`${path}: ${archive.length} bytes`);
}
