/**
 * The picking of what is under the pointer for 'hoverfeature', without stalling the GPU: the engine is asked once the
 * frame is drawn, its answer read on a later frame, when the GPU is done.
 */

import type { Layer } from '../layers/layers.ts';
import type { Point } from '../types.ts';
import type { Engine, PickResult } from '../wasm/isard.js';

/** Frames the hover picking waits for its answer at most (usually one) */
const MAX_POLLS = 30;

/** What 'hoverfeature' looks for: within `radius` CSS pixels, in `layers` only if given */
export interface HoverPicking {
  radius?: number;
  layers?: readonly Layer[];
}

/** The hover picking of a map: one request in flight at most, its answer polled frame after frame */
export class HoverPick {
  private readonly engine: () => Engine;
  /** The WebGL context is lost: no answer will come */
  private readonly lost: () => boolean;
  private readonly answer: (result: PickResult) => void;
  /** Frame polling the engine for the answer, 0 if none */
  private poll = 0;
  /** Polls under way: given up after `MAX_POLLS` (an answer that never comes) */
  private polls = 0;

  /** `engine`: the map's engine of the moment; `answer`: called with what the engine found */
  constructor(engine: () => Engine, lost: () => boolean, answer: (result: PickResult) => void) {
    this.engine = engine;
    this.lost = lost;
    this.answer = answer;
  }

  /** Asks the engine what is under the pointer; its answer, read without waiting for the GPU, comes on a later frame */
  request(point: Point, { radius = 0, layers }: HoverPicking): void {
    const only = layers && Uint32Array.from(layers.map((l) => l.id));

    // The engine cannot pick (no request in flight): the poll under way would wait for nothing
    if (!this.engine().requestPick(point[0], point[1], radius, only)) {
      this.cancel();

      return;
    }
    // A new request replaces the one in flight (in the engine too): one poll loop at most
    this.polls = 0;
    if (!this.poll) this.poll = requestAnimationFrame(this.next);
  }

  /** Stops waiting for the answer */
  cancel(): void {
    cancelAnimationFrame(this.poll);
    this.poll = 0;
  }

  private readonly next = (): void => {
    this.poll = 0;
    if (this.lost()) return;

    const result: PickResult | undefined = this.engine().pollPick();

    if (!result) {
      // The GPU is not done yet (usually a frame): asked again next frame, without drawing; given up at last
      this.polls += 1;
      if (this.polls < MAX_POLLS) this.poll = requestAnimationFrame(this.next);

      return;
    }
    this.answer(result);
  };
}
