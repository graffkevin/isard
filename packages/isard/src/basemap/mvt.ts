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
  /** An unsigned varint, approximated beyond 2^53; all its bytes are read (up to 10), so the next field is right */
  varint(): number {
    let value = 0;
    for (let shift = 0; shift < 70; shift += 7) {
      const b = this.buf[this.pos++] ?? 0;
      value += (b & 0x7f) * 2 ** shift;
      if (b < 0x80) break;
    }
    return value;
  }
  /** A signed 64-bit varint (two's complement: a negative value takes 10 bytes) */
  int64(): number {
    let lo = 0;
    let hi = 0;
    for (let i = 0; i < 10; i += 1) {
      const b = this.buf[this.pos++] ?? 0;
      const bits = b & 0x7f;
      // Bits 0-27 in the low word, bits 28-31 split, the rest in the high word
      if (i < 4) lo |= bits << (7 * i);
      else if (i === 4) {
        lo |= (bits & 0x0f) << 28;
        hi |= bits >> 4;
      } else hi |= bits << (7 * i - 32);
      if (b < 0x80) break;
    }
    // The high word signed: values from 2^63 up are negative
    return (hi | 0) * 2 ** 32 + (lo >>> 0);
  }
  sint(): number {
    const v = this.varint();
    return v % 2 === 1 ? -(v + 1) / 2 : v / 2;
  }
  /** A length-delimited field as a sub-reader */
  sub(): Pbf {
    const n = this.varint();
    // A length past the end (a truncated tile) stops at the end
    const end = Math.min(this.pos + n, this.end);
    const r = new Pbf(this.buf, this.pos, end);
    this.pos = end;
    return r;
  }
  string(): string {
    const n = this.varint();
    const end = Math.min(this.pos + n, this.end);
    const s = decoder.decode(this.buf.subarray(this.pos, end));
    this.pos = end;
    return s;
  }
  float(): number {
    const v = this.pos + 4 <= this.end ? this.view().getFloat32(this.pos, true) : 0;
    this.pos += 4;
    return v;
  }
  double(): number {
    const v = this.pos + 8 <= this.end ? this.view().getFloat64(this.pos, true) : 0;
    this.pos += 8;
    return v;
  }
  private view(): DataView {
    return new DataView(this.buf.buffer, this.buf.byteOffset, this.buf.byteLength);
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
        v = p.int64();
        break;
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
    // A count past the end (a truncated geometry) stops there
    for (let i = 0; i < count && p.more; i += 1) {
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
