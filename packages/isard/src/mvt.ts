/**
 * Mapbox Vector Tiles (MVT 2.1) decoding: layers of features with their properties and geometry, in tile units
 * (`extent`, usually 4096 across). Points are lists of points, lines lists of lines, polygons lists of rings (the
 * outer ones clockwise in tile coordinates, y down, holes the other way round, as the specification says).
 */

export type Value = string | number | boolean;

export interface TileFeature {
  /** 1 point, 2 line, 3 polygon */
  type: 1 | 2 | 3;
  properties: Record<string, Value>;
  /** Point lists: x0, y0, x1, y1… in tile units */
  geometry: number[][];
}

export interface TileLayer {
  extent: number;
  features: TileFeature[];
}

/** Protobuf reader of a buffer range */
class Pbf {
  pos: number;
  constructor(
    private readonly buf: Uint8Array,
    start = 0,
    readonly end = buf.length,
  ) {
    this.pos = start;
  }
  varint(): number {
    let value = 0;
    let shift = 0;
    for (;;) {
      const b = this.buf[this.pos++] ?? 0;
      value += (b & 0x7f) * 2 ** shift;
      if (b < 0x80 || shift > 49) return value;
      shift += 7;
    }
  }
  sint(): number {
    const v = this.varint();
    return v % 2 === 1 ? -(v + 1) / 2 : v / 2;
  }
  /** A length-delimited field as a sub-reader */
  sub(): Pbf {
    const n = this.varint();
    const r = new Pbf(this.buf, this.pos, this.pos + n);
    this.pos += n;
    return r;
  }
  string(): string {
    const n = this.varint();
    const s = decoder.decode(this.buf.subarray(this.pos, this.pos + n));
    this.pos += n;
    return s;
  }
  float(): number {
    const v = new DataView(this.buf.buffer, this.buf.byteOffset + this.pos, 4).getFloat32(0, true);
    this.pos += 4;
    return v;
  }
  double(): number {
    const v = new DataView(this.buf.buffer, this.buf.byteOffset + this.pos, 8).getFloat64(0, true);
    this.pos += 8;
    return v;
  }
  skip(wire: number): void {
    if (wire === 0) this.varint();
    else if (wire === 1) this.pos += 8;
    else if (wire === 2) this.pos += this.varint();
    else if (wire === 5) this.pos += 4;
  }
  get more(): boolean {
    return this.pos < this.end;
  }
}

const decoder = new TextDecoder();

function value(p: Pbf): Value {
  let v: Value = '';
  while (p.more) {
    const tag = p.varint();
    switch (tag >> 3) {
      case 1:
        v = p.string();
        break;
      case 2:
        v = p.float();
        break;
      case 3:
        v = p.double();
        break;
      case 4:
      case 5:
        v = p.varint();
        break;
      case 6:
        v = p.sint();
        break;
      case 7:
        v = p.varint() !== 0;
        break;
      default:
        p.skip(tag & 7);
    }
  }
  return v;
}

/** Point lists of a geometry's commands (MoveTo, LineTo, ClosePath) */
function geometry(p: Pbf): number[][] {
  const out: number[][] = [];
  let current: number[] | undefined;
  let [x, y] = [0, 0];
  while (p.more) {
    const command = p.varint();
    const id = command & 7;
    const count = command >> 3;
    if (id === 7) {
      // ClosePath: back to the first point
      if (current && current.length >= 2) current.push(current[0] ?? 0, current[1] ?? 0);
      continue;
    }
    for (let i = 0; i < count; i += 1) {
      x += p.sint();
      y += p.sint();
      if (id === 1) {
        current = [];
        out.push(current);
      }
      current?.push(x, y);
    }
  }
  return out;
}

/** Decodes a tile; only the layers in `wanted` (all if undefined) */
export function decode(bytes: Uint8Array, wanted?: Set<string>): Map<string, TileLayer> {
  const layers = new Map<string, TileLayer>();
  const tile = new Pbf(bytes);
  while (tile.more) {
    const tag = tile.varint();
    if (tag >> 3 !== 3) {
      tile.skip(tag & 7);
      continue;
    }
    const l = tile.sub();
    let name = '';
    let extent = 4096;
    const keys: string[] = [];
    const values: Value[] = [];
    const raw: Pbf[] = [];
    while (l.more) {
      const t = l.varint();
      switch (t >> 3) {
        case 1:
          name = l.string();
          break;
        case 2:
          raw.push(l.sub());
          break;
        case 3:
          keys.push(l.string());
          break;
        case 4:
          values.push(value(l.sub()));
          break;
        case 5:
          extent = l.varint();
          break;
        default:
          l.skip(t & 7);
      }
    }
    if (wanted && !wanted.has(name)) continue;
    const features: TileFeature[] = [];
    for (const f of raw) {
      let type: 1 | 2 | 3 = 1;
      const properties: Record<string, Value> = {};
      let geom: number[][] = [];
      while (f.more) {
        const t = f.varint();
        switch (t >> 3) {
          case 2: {
            const tags = f.sub();
            while (tags.more) {
              const k = keys[tags.varint()];
              const v = values[tags.varint()];
              if (k !== undefined && v !== undefined) properties[k] = v;
            }
            break;
          }
          case 3:
            type = Math.min(3, Math.max(1, f.varint())) as 1 | 2 | 3;
            break;
          case 4:
            geom = geometry(f.sub());
            break;
          default:
            f.skip(t & 7);
        }
      }
      features.push({ type, properties, geometry: geom });
    }
    layers.set(name, { extent, features });
  }
  return layers;
}
