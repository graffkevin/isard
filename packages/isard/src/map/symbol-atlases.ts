/** The atlases of a map's symbol layers, the glyphs and the icons: created on first use, moved to each new engine. */

import { Glyphs } from '../layers/glyphs.ts';
import { Icons, type ImageOptions, type Pixels, readPixels } from '../layers/icons.ts';
import type { Engine } from '../wasm/isard.js';

/** The largest texture the canvas's WebGL context takes, undefined if unknown: the atlases stay within it */
const maxTextureSize = (canvas: HTMLCanvasElement): number | undefined => {
  const gl = canvas.getContext('webgl2');
  const max = gl?.getParameter(gl.MAX_TEXTURE_SIZE);

  return typeof max === 'number' ? max : undefined;
};

/** The glyphs and icons of a map */
export class SymbolAtlases {
  private readonly canvas: HTMLCanvasElement;
  private readonly glyphsUrl: string | undefined;
  private readonly engine: () => Engine;
  #glyphs: Glyphs | undefined;
  #icons: Icons | undefined;

  /** `canvas`: the engine's; `glyphsUrl`: the map's option; `engine`: the map's engine of the moment */
  constructor(canvas: HTMLCanvasElement, glyphsUrl: string | undefined, engine: () => Engine) {
    this.canvas = canvas;
    this.glyphsUrl = glyphsUrl;
    this.engine = engine;
  }

  /** The glyphs of the symbol layers, loaded on first use */
  get glyphs(): Glyphs {
    if (!this.#glyphs) {
      // The engine's context: the atlas stays within its texture size limit
      this.#glyphs = new Glyphs(this.glyphsUrl, maxTextureSize(this.canvas));
      this.#glyphs.attach(this.engine());
    }

    return this.#glyphs;
  }

  /** The icons of the symbol layers: the application's images and the vector basemaps' sprites */
  get icons(): Icons {
    if (!this.#icons) {
      this.#icons = new Icons(maxTextureSize(this.canvas));
      this.#icons.attach(this.engine());
    }

    return this.#icons;
  }

  /** An image for the icons, from its pixels or from an image; one without pixels throws */
  addImage(
    name: string,
    image: Pixels | (CanvasImageSource & { width: number; height: number }),
    options: ImageOptions,
  ): void {
    const pixels = 'data' in image ? image : readPixels(image);

    if (!pixels) throw new Error(`isard: the image ${name} has no pixels`);
    this.icons.add(name, pixels, options);
  }

  hasImage(name: string): boolean {
    return this.#icons?.has(name) ?? false;
  }

  removeImage(name: string): void {
    this.#icons?.remove(name);
  }

  /** Uploads the atlases made so far to a new engine (after a context loss, or the one left once the map is gone) */
  attach(engine: Engine): void {
    this.#glyphs?.attach(engine);
    this.#icons?.attach(engine);
  }
}
