/**
 * Glyphs for the symbol layers: signed distance fields in MapLibre's glyph format (ranges of 256 code points per
 * protobuf file, 24 px glyphs with a 3 px buffer), loaded from the Géoplateforme's fonts on first use, packed into one
 * atlas the engine samples; and the shaping of a label: its lines (explicit and wrapped), their glyphs placed around
 * the anchor.
 */
import type { Anchor } from './overlay.ts';
import type { Engine } from './wasm/isard.js';

/** Fonts of the Géoplateforme's vector styles (Source Sans Pro Regular, Bold, Italic, Semibold) */
export const GLYPHS_URL = 'https://data.geopf.fr/annexes/ressources/vectorTiles/fonts/{fontstack}/{range}.pbf';

/** Size the glyphs are drawn at in the files, and their buffer */
const EM = 24;
const BUFFER = 3;

export interface Glyph {
  width: number;
  height: number;
  left: number;
  top: number;
  advance: number;
  /** Place of its bitmap ((width + 6) × (height + 6)) in the atlas; undefined for an empty one (a space) */
  x?: number;
  y?: number;
}

/** A protobuf reader, enough for the glyph files */
class Reader {
  pos = 0;
  constructor(private readonly buf: Uint8Array) {}
  get done(): boolean {
    return this.pos >= this.buf.length;
  }
  varint(): number {
    let value = 0;
    let shift = 0;
    for (;;) {
      const b = this.buf[this.pos++] ?? 0;
      value += (b & 0x7f) * 2 ** shift;
      if (b < 0x80) return value;
      shift += 7;
    }
  }
  sint(): number {
    const v = this.varint();
    return v % 2 === 1 ? -(v + 1) / 2 : v / 2;
  }
  bytes(): Uint8Array {
    const n = this.varint();
    const out = this.buf.subarray(this.pos, this.pos + n);
    this.pos += n;
    return out;
  }
  skip(wire: number): void {
    if (wire === 0) this.varint();
    else if (wire === 2) this.pos += this.varint();
    else if (wire === 5) this.pos += 4;
    else if (wire === 1) this.pos += 8;
  }
}

/** Glyphs of a range file: id → metrics and bitmap */
function parse(buffer: ArrayBuffer): Map<number, { glyph: Glyph; bitmap: Uint8Array }> {
  const out = new Map<number, { glyph: Glyph; bitmap: Uint8Array }>();
  const root = new Reader(new Uint8Array(buffer));
  while (!root.done) {
    const tag = root.varint();
    if (tag >> 3 !== 1) {
      root.skip(tag & 7);
      continue;
    }
    // A fontstack
    const stack = new Reader(root.bytes());
    while (!stack.done) {
      const t = stack.varint();
      if (t >> 3 !== 3) {
        stack.skip(t & 7);
        continue;
      }
      const g = new Reader(stack.bytes());
      let id = 0;
      let bitmap: Uint8Array = new Uint8Array(0);
      const glyph: Glyph = { width: 0, height: 0, left: 0, top: 0, advance: 0 };
      while (!g.done) {
        const f = g.varint();
        switch (f >> 3) {
          case 1:
            id = g.varint();
            break;
          case 2:
            bitmap = g.bytes();
            break;
          case 3:
            glyph.width = g.varint();
            break;
          case 4:
            glyph.height = g.varint();
            break;
          case 5:
            glyph.left = g.sint();
            break;
          case 6:
            glyph.top = g.sint();
            break;
          case 7:
            glyph.advance = g.varint();
            break;
          default:
            g.skip(f & 7);
        }
      }
      out.set(id, { glyph, bitmap });
    }
  }
  return out;
}

/** The glyphs of a map and their atlas */
export class Glyphs {
  private size = 512;
  private atlas = new Uint8Array(512 * 512);
  /** Shelves of the packer: y, height, x of the next free place */
  private shelves: { y: number; height: number; x: number }[] = [];
  private readonly fonts = new Map<string, Map<number, Glyph>>();
  private readonly ranges = new Map<string, Promise<void>>();
  private engine: Engine | undefined;

  constructor(private readonly url = GLYPHS_URL) {}

  /** Sends the whole atlas to an engine (a new one after a context loss) */
  attach(engine: Engine): void {
    this.engine = engine;
    engine.setGlyphAtlas(this.size, this.size, this.atlas);
  }

  get atlasSize(): number {
    return this.size;
  }

  /** Loads the glyphs of these code points in a font */
  async load(font: string, codes: Iterable<number>): Promise<Map<number, Glyph>> {
    const glyphs = this.fonts.get(font) ?? new Map<number, Glyph>();
    this.fonts.set(font, glyphs);
    const starts = new Set<number>();
    for (const code of codes) if (!glyphs.has(code)) starts.add(Math.floor(code / 256) * 256);
    await Promise.all([...starts].map((start) => this.range(font, start, glyphs)));
    return glyphs;
  }

  private range(font: string, start: number, glyphs: Map<number, Glyph>): Promise<void> {
    const key = `${font}/${start}`;
    let pending = this.ranges.get(key);
    if (!pending) {
      const url = this.url
        .replace('{fontstack}', encodeURIComponent(font))
        .replace('{range}', `${start}-${start + 255}`);
      pending = fetch(url)
        .then((r) => (r.ok ? r.arrayBuffer() : Promise.reject(new Error(`${url}: ${r.status}`))))
        .then((buffer) => {
          for (const [id, { glyph, bitmap }] of parse(buffer)) {
            if (glyph.width > 0 && bitmap.length >= (glyph.width + 2 * BUFFER) * (glyph.height + 2 * BUFFER)) {
              this.pack(glyph, bitmap);
            }
            glyphs.set(id, glyph);
          }
        })
        .catch((error: unknown) => {
          // A missing range is not retried this session; its characters stay blank
          console.warn('isard: glyphs', error);
        });
      this.ranges.set(key, pending);
    }
    return pending;
  }

  /** Places a bitmap in the atlas (growing it if full) and sends it to the engine */
  private pack(glyph: Glyph, bitmap: Uint8Array): void {
    const w = glyph.width + 2 * BUFFER;
    const h = glyph.height + 2 * BUFFER;
    // One texel of padding between bitmaps: linear filtering never reads a neighbour
    let shelf = this.shelves.find((s) => s.height >= h && s.height <= h + 4 && s.x + w + 1 <= this.size);
    if (!shelf) {
      const last = this.shelves.at(-1);
      const y = last ? last.y + last.height + 1 : 0;
      if (y + h > this.size) {
        this.grow();
        this.pack(glyph, bitmap);
        return;
      }
      shelf = { y, height: h, x: 0 };
      this.shelves.push(shelf);
    }
    glyph.x = shelf.x;
    glyph.y = shelf.y;
    shelf.x += w + 1;
    for (let row = 0; row < h; row += 1) {
      this.atlas.set(bitmap.subarray(row * w, row * w + w), (glyph.y + row) * this.size + glyph.x);
    }
    this.engine?.updateGlyphAtlas(glyph.x, glyph.y, w, h, bitmap.subarray(0, w * h));
  }

  /** Doubles the atlas; the glyphs keep their places (texel coordinates) */
  private grow(): void {
    const size = this.size * 2;
    const atlas = new Uint8Array(size * size);
    for (let row = 0; row < this.size; row += 1) {
      atlas.set(this.atlas.subarray(row * this.size, (row + 1) * this.size), row * size);
    }
    this.size = size;
    this.atlas = atlas;
    this.engine?.setGlyphAtlas(size, size, atlas);
  }
}

/** A run of text with its own scale (MapLibre's `format` with `font-scale`) and font */
export interface TextPart {
  text: string;
  scale?: number;
  font?: string;
}

export type { Anchor } from './overlay.ts';

export interface Shaping {
  /** Glyph quads: x, y, width, height (CSS pixels from the anchor), atlas u0, v0, u1, v1, font scale */
  quads: number[][];
  /** Ink box from the anchor: min x, min y, max x, max y */
  box: [number, number, number, number];
}

/** Code points of a text, for loading */
export function codes(parts: readonly TextPart[]): Set<number> {
  const out = new Set<number>();
  for (const p of parts) for (const ch of p.text) out.add(ch.codePointAt(0) ?? 32);
  return out;
}

/**
 * Lays out a label: lines split at `\n` and wrapped at `maxWidth` ems, centered, the block placed by `anchor` around
 * the anchor point and moved by `offset` (ems); `size` in CSS pixels. Glyphs not loaded take no room.
 */
export function shape(
  parts: readonly TextPart[],
  glyphsOf: (font: string) => Map<number, Glyph> | undefined,
  options: {
    font: string;
    size: number;
    maxWidth: number;
    lineHeight: number;
    anchor: Anchor;
    offset: [number, number];
  },
): Shaping {
  const { size, maxWidth, lineHeight, anchor, offset } = options;
  const k = size / EM;
  // Characters with their glyph and scale, split into lines
  type Char = { glyph: Glyph | undefined; scale: number; space: boolean };
  const lines: Char[][] = [[]];
  for (const part of parts) {
    const glyphs = glyphsOf(part.font ?? options.font);
    for (const ch of part.text) {
      if (ch === '\n') {
        lines.push([]);
        continue;
      }
      const code = ch.codePointAt(0) ?? 32;
      lines.at(-1)?.push({ glyph: glyphs?.get(code), scale: part.scale ?? 1, space: /\s/.test(ch) });
    }
  }
  const advance = (c: Char) => (c.glyph?.advance ?? (c.space ? EM / 4 : 0)) * c.scale;
  // Wrapping at spaces, greedy
  const limit = maxWidth * EM;
  const wrapped: Char[][] = [];
  for (const line of lines) {
    let current: Char[] = [];
    let width = 0;
    let lastSpace = -1;
    for (const c of line) {
      current.push(c);
      width += advance(c);
      if (c.space) lastSpace = current.length - 1;
      if (width > limit && lastSpace > 0) {
        wrapped.push(current.slice(0, lastSpace));
        current = current.slice(lastSpace + 1);
        width = current.reduce((s, x) => s + advance(x), 0);
        lastSpace = -1;
      }
    }
    wrapped.push(current);
  }
  // Glyphs on their lines, each line centered on x = 0, in units of the 24 px font
  const quads: number[][] = [];
  let [x0, y0, x1, y1] = [
    Number.POSITIVE_INFINITY,
    Number.POSITIVE_INFINITY,
    Number.NEGATIVE_INFINITY,
    Number.NEGATIVE_INFINITY,
  ];
  let baseline = 0;
  wrapped.forEach((line, i) => {
    const scale = Math.max(1, ...line.map((c) => c.scale));
    if (i > 0) baseline += lineHeight * EM * scale;
    const width = line.reduce((s, c) => s + advance(c), 0);
    let pen = -width / 2;
    for (const c of line) {
      const g = c.glyph;
      if (g && g.x !== undefined && g.y !== undefined && g.width > 0) {
        const qx = pen + (g.left - BUFFER) * c.scale;
        const qy = baseline - (g.top + BUFFER) * c.scale;
        const qw = (g.width + 2 * BUFFER) * c.scale;
        const qh = (g.height + 2 * BUFFER) * c.scale;
        quads.push([qx, qy, qw, qh, g.x, g.y, g.x + g.width + 2 * BUFFER, g.y + g.height + 2 * BUFFER, c.scale]);
        x0 = Math.min(x0, qx + BUFFER * c.scale);
        y0 = Math.min(y0, qy + BUFFER * c.scale);
        x1 = Math.max(x1, qx + qw - BUFFER * c.scale);
        y1 = Math.max(y1, qy + qh - BUFFER * c.scale);
      }
      pen += advance(c);
    }
  });
  if (!quads.length) return { quads, box: [0, 0, 0, 0] };
  // The block placed around the anchor by its ink box
  // The side of the box at the anchor: its start, its end, or its middle
  const side = (start: boolean, end: boolean, lo: number, hi: number) => {
    if (start) return -lo;
    if (end) return -hi;
    return -(lo + hi) / 2;
  };
  const horizontal = side(anchor.includes('left'), anchor.includes('right'), x0, x1);
  const vertical = side(anchor.startsWith('top'), anchor.startsWith('bottom'), y0, y1);
  const dx = horizontal + offset[0] * EM;
  const dy = vertical + offset[1] * EM;
  for (const q of quads) {
    q[0] = ((q[0] ?? 0) + dx) * k;
    q[1] = ((q[1] ?? 0) + dy) * k;
    q[2] = (q[2] ?? 0) * k;
    q[3] = (q[3] ?? 0) * k;
    q[8] = (q[8] ?? 1) * k;
  }
  return { quads, box: [(x0 + dx) * k, (y0 + dy) * k, (x1 + dx) * k, (y1 + dy) * k] };
}
