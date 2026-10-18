/**
 * Icons for the symbol layers: the images of a style's sprite (MapLibre's sprite format: a JSON index and a PNG sheet,
 * `@2x` on high-density screens, one sprite or several with their ids) and the application's own (`map.addImage`),
 * packed on first use into an RGBA atlas the engine samples (premultiplied: no dark fringe where it is scaled); and the
 * layout of an icon around its anchor: by its anchor and offset, or stretched around its text (`icon-text-fit`), by
 * the stretchable zones and the content box of its image when it has them (`stretchX`, `stretchY`, `content`).
 */
import type { Anchor } from './overlay.ts';
import type { Engine } from './wasm/isard.js';

/** An image of a sprite's index: its place in the sheet (pixels) and how it is drawn */
export interface SpriteIcon {
  x: number;
  y: number;
  width: number;
  height: number;
  /** Image pixels per CSS pixel (default 1; 2 in an `@2x` sheet) */
  pixelRatio?: number;
  /** A signed distance field in alpha, drawn in the icon color */
  sdf?: boolean;
  /** Columns and rows that stretch when the icon is fitted to its text (image pixels): the others keep their size */
  stretchX?: [number, number][];
  stretchY?: [number, number][];
  /** Where the text goes when the icon is fitted to it: left, top, right, bottom (image pixels) */
  content?: [number, number, number, number];
}

export type SpriteIndex = Record<string, SpriteIcon>;

/** RGBA pixels, straight alpha, row after row */
export interface Pixels {
  width: number;
  height: number;
  data: Uint8Array | Uint8ClampedArray;
}

/** How an image of the application is drawn (`map.addImage`): `SpriteIcon` without its place */
export type ImageOptions = Omit<SpriteIcon, 'x' | 'y' | 'width' | 'height'>;

/** An icon in the atlas: its place (texels) and how it is drawn */
export interface Icon {
  x: number;
  y: number;
  width: number;
  height: number;
  pixelRatio: number;
  sdf: boolean;
  stretchX: [number, number][] | undefined;
  stretchY: [number, number][] | undefined;
  content: [number, number, number, number] | undefined;
}

/** A sprite of a style: its id (`default`: its images keep their names, else `id:name`) and its URL without
 * extension */
export interface SpriteSource {
  id: string;
  url: string;
}

/** The sprites of a style's `sprite`: one URL, or several with their ids (MapLibre's style specification) */
export function spriteSources(sprite: string | readonly SpriteSource[] | undefined): SpriteSource[] {
  if (typeof sprite === 'string') return sprite ? [{ id: 'default', url: sprite }] : [];
  return Array.isArray(sprite) ? sprite.filter((s) => typeof s?.url === 'string' && typeof s.id === 'string') : [];
}

/** The name of an image of a sprite in the style: its own in the `default` sprite, `id:name` in another */
export function spriteName(id: string, name: string): string {
  return id === 'default' ? name : `${id}:${name}`;
}

/** The URL of a sprite's index or sheet: `.json` or `.png`, `@2x` before it for high-density screens, before the
 * query of the URL if it has one */
export function spriteFile(url: string, extension: 'json' | 'png', ratio: number): string {
  const suffix = `${ratio > 1 ? '@2x' : ''}.${extension}`;
  const query = url.search(/[?#]/);
  return query < 0 ? `${url}${suffix}` : `${url.slice(0, query)}${suffix}${url.slice(query)}`;
}

/** A sprite loaded: its images by name (with the sprite's prefix) and its sheet */
export interface LoadedSprite {
  icons: SpriteIndex;
  image: ImageBitmap;
}

/** Loads a style's sprites (`@2x` above a device pixel ratio of 1, the 1x ones if a server has none); the sprites
 * that fail are left out */
export async function loadSprites(
  sprite: string | readonly SpriteSource[] | undefined,
  ratio = typeof devicePixelRatio === 'number' ? devicePixelRatio : 1,
): Promise<LoadedSprite[]> {
  const one = async ({ id, url }: SpriteSource, r: number): Promise<LoadedSprite | undefined> => {
    const [json, png] = await Promise.all([fetch(spriteFile(url, 'json', r)), fetch(spriteFile(url, 'png', r))]);
    if (!json.ok || !png.ok) return r > 1 ? one({ id, url }, 1) : undefined;
    const index = (await json.json()) as SpriteIndex;
    const icons: SpriteIndex = {};
    for (const [name, icon] of Object.entries(index)) icons[spriteName(id, name)] = icon;
    return { icons, image: await createImageBitmap(await png.blob()) };
  };
  const loaded = await Promise.all(spriteSources(sprite).map((s) => one(s, ratio).catch(() => undefined)));
  return loaded.filter((s): s is LoadedSprite => s !== undefined);
}

/** The pixels of an image (read back through a canvas) */
export function readPixels(image: CanvasImageSource & { width: number; height: number }): Pixels | undefined {
  const { width, height } = image;
  if (!(width > 0 && height > 0)) return undefined;
  const canvas =
    typeof OffscreenCanvas === 'function'
      ? new OffscreenCanvas(width, height)
      : Object.assign(document.createElement('canvas'), { width, height });
  const g = canvas.getContext('2d') as CanvasRenderingContext2D | OffscreenCanvasRenderingContext2D | null;
  if (!g) return undefined;
  g.drawImage(image, 0, 0);
  return g.getImageData(0, 0, width, height);
}

/** Where an icon's pixels are: a region of an image, read when first packed */
interface IconSource {
  pixels: () => Pixels | undefined;
  x: number;
  y: number;
  width: number;
  height: number;
  options: ImageOptions;
}

/** A shelf of the packer: its y, height, and the x of the next free place */
interface Shelf {
  y: number;
  height: number;
  x: number;
}

/** The icons of a map and their atlas: a square that doubles as it fills, up to the texture size limit */
export class Icons {
  private size = 256;
  private data = new Uint8Array(256 * 256 * 4);
  private shelves: Shelf[] = [];
  private readonly sources = new Map<string, IconSource>();
  /** Icons packed, null for those that cannot be (no pixels, the atlas full) */
  private readonly packed = new Map<string, Icon | null>();
  private engine: Engine | undefined;
  /** Rows changed since the last upload: first, end */
  private dirty: [number, number] | undefined;
  private full = false;
  private readonly maxSize: number;
  /** Bumped each time icons are added or removed: labels laid out without some of theirs are laid out again */
  version = 0;

  /** `maxSize`: the GPU's texture size limit if known (the atlas is at most 4096 × 4096) */
  constructor(maxSize = 4096) {
    this.maxSize = Math.max(256, Math.min(4096, maxSize));
  }

  /** Sends the whole atlas to an engine (a new one after a context loss) */
  attach(engine: Engine): void {
    this.engine = engine;
    this.dirty = undefined;
    this.upload();
  }

  /** Side of the atlas */
  get atlasSize(): number {
    return this.size;
  }

  /** An image of the application under `name` (another image of that name is replaced) */
  add(name: string, pixels: Pixels, options: ImageOptions = {}): void {
    this.define(name, { pixels: () => pixels, x: 0, y: 0, width: pixels.width, height: pixels.height, options });
  }

  /** The images of a sprite's index, `prefix` before their names, their pixels read from the sheet on first use */
  addSprite(prefix: string, index: SpriteIndex, sheet: () => Pixels | undefined): void {
    let pixels: Pixels | undefined;
    let read = false;
    const once = () => {
      if (!read) {
        read = true;
        pixels = sheet();
      }
      return pixels;
    };
    for (const [name, icon] of Object.entries(index)) {
      if (!icon || typeof icon !== 'object') continue;
      const { x, y, width, height, ...options } = icon;
      this.define(`${prefix}${name}`, { pixels: once, x, y, width, height, options }, false);
    }
    this.version += 1;
  }

  private define(name: string, source: IconSource, bump = true): void {
    this.sources.set(name, source);
    // Packed again on its next use (its old place stays taken)
    this.packed.delete(name);
    if (bump) this.version += 1;
  }

  has(name: string): boolean {
    return this.sources.has(name);
  }

  /** Forgets an image, or every image whose name starts with `prefix` (a sprite's) */
  remove(name: string, prefix = false): void {
    for (const key of [...this.sources.keys()]) {
      if (prefix ? key.startsWith(name) : key === name) {
        this.sources.delete(key);
        this.packed.delete(key);
      }
    }
    this.version += 1;
  }

  /** The icon of this name, packed into the atlas on first use; undefined if there is none (`flush` uploads it) */
  get(name: string): Icon | undefined {
    const hit = this.packed.get(name);
    if (hit !== undefined) return hit ?? undefined;
    const source = this.sources.get(name);
    if (!source) return undefined;
    const icon = this.pack(source);
    this.packed.set(name, icon ?? null);
    return icon;
  }

  /** Sends the rows changed since the last upload to the engine */
  flush(): void {
    const dirty = this.dirty;
    this.dirty = undefined;
    if (!dirty || !this.engine) return;
    const [y0, y1] = dirty;
    const row = this.size * 4;
    this.engine.updateIconAtlas(0, y0, this.size, y1 - y0, this.data.subarray(y0 * row, y1 * row));
  }

  private upload(): void {
    const engine = this.engine;
    if (!engine) return;
    engine.setIconAtlas(this.size, this.size);
    const used = this.shelves.reduce((m, s) => Math.max(m, s.y + s.height), 0);
    if (used > 0) engine.updateIconAtlas(0, 0, this.size, used, this.data.subarray(0, used * this.size * 4));
  }

  /** Copies an icon's pixels into the atlas, premultiplied, with a texel of room around it */
  private pack(source: IconSource): Icon | undefined {
    const pixels = source.pixels();
    const { x, y, width: w, height: h } = source;
    if (!pixels || !(w > 0 && h > 0) || x < 0 || y < 0 || x + w > pixels.width || y + h > pixels.height) {
      return undefined;
    }
    const place = this.place(w + 2, h + 2);
    if (!place) return undefined;
    const [px, py] = [place[0] + 1, place[1] + 1];
    for (let row = 0; row < h; row += 1) {
      for (let col = 0; col < w; col += 1) {
        const from = ((y + row) * pixels.width + x + col) * 4;
        const to = ((py + row) * this.size + px + col) * 4;
        const a = pixels.data[from + 3] ?? 0;
        this.data[to] = ((pixels.data[from] ?? 0) * a + 127) / 255;
        this.data[to + 1] = ((pixels.data[from + 1] ?? 0) * a + 127) / 255;
        this.data[to + 2] = ((pixels.data[from + 2] ?? 0) * a + 127) / 255;
        this.data[to + 3] = a;
      }
    }
    const [d0, d1] = this.dirty ?? [place[1], place[1]];
    this.dirty = [Math.min(d0, place[1]), Math.max(d1, place[1] + h + 2)];
    const { pixelRatio, sdf, stretchX, stretchY, content } = source.options;
    return {
      x: px,
      y: py,
      width: w,
      height: h,
      pixelRatio: pixelRatio && pixelRatio > 0 ? pixelRatio : 1,
      sdf: sdf === true,
      stretchX: Array.isArray(stretchX) ? stretchX : undefined,
      stretchY: Array.isArray(stretchY) ? stretchY : undefined,
      content: Array.isArray(content) && content.length === 4 ? content : undefined,
    };
  }

  /** A free place of `w` × `h` texels (shelves, the atlas doubled when full); undefined once it cannot grow */
  private place(w: number, h: number): [number, number] | undefined {
    for (;;) {
      const shelf = this.shelves.find((s) => s.height >= h && s.height <= h + 8 && s.x + w <= this.size);
      if (shelf) {
        shelf.x += w;
        return [shelf.x - w, shelf.y];
      }
      const last = this.shelves.at(-1);
      const y = last ? last.y + last.height : 0;
      if (y + h <= this.size && w <= this.size) {
        this.shelves.push({ y, height: h, x: w });
        return [0, y];
      }
      if (this.size * 2 > this.maxSize) {
        if (!this.full) console.warn(`isard: the icon atlas is full (${this.size} × ${this.size})`);
        this.full = true;
        return undefined;
      }
      this.grow();
    }
  }

  /** Doubles the atlas; the icons keep their places (texels) */
  private grow(): void {
    const size = this.size * 2;
    const data = new Uint8Array(size * size * 4);
    for (let row = 0; row < this.size; row += 1) {
      data.set(this.data.subarray(row * this.size * 4, (row + 1) * this.size * 4), row * size * 4);
    }
    this.size = size;
    this.data = data;
    this.dirty = undefined;
    this.upload();
  }
}

/** How an icon is laid out around its anchor */
export interface IconLayoutOptions {
  /** Scale of the image (`icon-size`) */
  size: number;
  anchor: Anchor;
  /** CSS pixels at size 1 */
  offset: [number, number];
  /** Degrees, clockwise */
  rotate: number;
  /** Stretched around the text: its width, its height, both (`icon-text-fit`) */
  fit: 'none' | 'width' | 'height' | 'both';
  /** Room around the text when fitted: top, right, bottom, left (CSS pixels) */
  padding: [number, number, number, number];
  /** The text's box around the anchor (CSS pixels), if it has one */
  text?: [number, number, number, number] | undefined;
}

/** An icon laid out: its quads (x, y, width, height in CSS pixels from the anchor, atlas u0, v0, u1, v1 in texels, -1
 * or -2 for a distance field, turn in radians) and its box from the anchor */
export interface IconLayout {
  quads: number[][];
  box: [number, number, number, number];
}

/** Pieces of an image along one axis: image start, end, laid out start, end */
type Pieces = [number, number, number, number][];

/**
 * An axis of an icon fitted to `lo`…`hi` (CSS pixels): its content (the whole image by default) spans them, its
 * stretchable zones (the whole image by default) taking the room the fixed parts (at `scale` CSS pixels per image
 * pixel) leave
 */
export function stretch(
  length: number,
  zones: readonly [number, number][] | undefined,
  content: [number, number] | undefined,
  lo: number,
  hi: number,
  scale: number,
): Pieces {
  const clamp = (v: number) => Math.max(0, Math.min(length, Number(v) || 0));
  const spans = (zones?.length ? zones : [[0, length] as [number, number]])
    .map(([a, b]): [number, number] => [clamp(Math.min(a, b)), clamp(Math.max(a, b))])
    .filter(([a, b]) => b > a);
  const cuts = [...new Set([0, length, ...spans.flat()])].sort((a, b) => a - b);
  const pieces = cuts.slice(1).map((b, i): [number, number, boolean] => {
    const a = cuts[i] ?? 0;
    return [a, b, spans.some(([s, e]) => s <= a && b <= e)];
  });
  const [c0, c1] = content ? [clamp(content[0]), clamp(content[1])] : [0, length];
  const overlap = (a: number, b: number) => Math.max(0, Math.min(b, c1) - Math.max(a, c0));
  const stretchable = pieces.reduce((n, [a, b, s]) => n + (s ? overlap(a, b) : 0), 0);
  const fixed = c1 - c0 - stretchable;
  // CSS pixels per stretchable image pixel: what the fixed parts of the content leave of the room
  const k = stretchable > 0 ? Math.max(0, (hi - lo - fixed * scale) / stretchable) : scale;
  // Laid out position of an image coordinate
  const at = (x: number) => pieces.reduce((n, [a, b, s]) => n + Math.max(0, Math.min(x, b) - a) * (s ? k : scale), 0);
  const origin = lo - at(c0);
  return pieces.map(([a, b]) => [a, b, origin + at(a), origin + at(b)]);
}

/** Where an anchor's side lies along an axis of the icon: at its start 0, at its end 1, else the middle */
function align(start: boolean, end: boolean): number {
  if (start) return 0;
  return end ? 1 : 0.5;
}

/** Lays an icon out around its anchor: placed by its anchor and offset, or fitted to its text */
export function layoutIcon(icon: Icon, options: IconLayoutOptions): IconLayout {
  const { size, anchor, offset, rotate, fit, padding, text } = options;
  const { width: w, height: h, pixelRatio } = icon;
  let xs: Pieces;
  let ys: Pieces;
  if (fit !== 'none' && text) {
    const [top, right, bottom, left] = padding;
    const [x0, y0, x1, y1] = [text[0] - left, text[1] - top, text[2] + right, text[3] + bottom];
    // The fixed parts at the image's own size; an axis not fitted at the icon's size, centered on the text
    const own = 1 / pixelRatio;
    const scaled = size / pixelRatio;
    const centered = (length: number, lo: number, hi: number): Pieces => {
      const c = (lo + hi) / 2;
      return [[0, length, c - (length * scaled) / 2, c + (length * scaled) / 2]];
    };
    const content = icon.content;
    xs =
      fit === 'height'
        ? centered(w, x0, x1)
        : stretch(w, icon.stretchX, content && [content[0], content[2]], x0, x1, own);
    ys =
      fit === 'width'
        ? centered(h, y0, y1)
        : stretch(h, icon.stretchY, content && [content[1], content[3]], y0, y1, own);
  } else {
    const [dw, dh] = [(w / pixelRatio) * size, (h / pixelRatio) * size];
    const ax = align(anchor.includes('left'), anchor.includes('right'));
    const ay = align(anchor.startsWith('top'), anchor.startsWith('bottom'));
    const [left, top] = [offset[0] * size - dw * ax, offset[1] * size - dh * ay];
    xs = [[0, w, left, left + dw]];
    ys = [[0, h, top, top + dh]];
  }
  const angle = ((Number(rotate) || 0) * Math.PI) / 180;
  const kind = icon.sdf ? -2 : -1;
  const quads: number[][] = [];
  const [cs, sn] = [Math.cos(angle), Math.sin(angle)];
  const box: [number, number, number, number] = [
    Number.POSITIVE_INFINITY,
    Number.POSITIVE_INFINITY,
    Number.NEGATIVE_INFINITY,
    Number.NEGATIVE_INFINITY,
  ];
  for (const [v0, v1, top, bottom] of ys) {
    for (const [u0, u1, left, right] of xs) {
      if (right <= left || bottom <= top) continue;
      quads.push([
        left,
        top,
        right - left,
        bottom - top,
        icon.x + u0,
        icon.y + v0,
        icon.x + u1,
        icon.y + v1,
        kind,
        angle,
      ]);
      // The box of the turned corners
      for (const [px, py] of [
        [left, top],
        [right, top],
        [left, bottom],
        [right, bottom],
      ] as const) {
        const [rx, ry] = [px * cs - py * sn, px * sn + py * cs];
        box[0] = Math.min(box[0], rx);
        box[1] = Math.min(box[1], ry);
        box[2] = Math.max(box[2], rx);
        box[3] = Math.max(box[3], ry);
      }
    }
  }
  return { quads, box: quads.length ? box : [0, 0, 0, 0] };
}
