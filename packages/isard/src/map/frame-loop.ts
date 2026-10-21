/**
 * The frame loop of a map: frames drawn on demand only (an idle map costs nothing), the running animation stepped in
 * each, and the events of a frame sent with it: the camera's moves, the pointer's position (coalesced to one per
 * frame, so the values computed for it are drawn in the same frame), 'render', and 'idle' once everything is loaded.
 */

import type { Point } from '../types.ts';
import type { Step } from '../util/animation.ts';
import type { Camera, Engine } from '../wasm/isard.js';
import type { MapListeners } from './events.ts';

/** What the frame loop needs of its map */
export interface FrameHost {
  /** The map's engine of the moment */
  engine(): Engine;
  /** The map's camera */
  camera(): Camera;
  /** The WebGL context is lost: nothing is drawn */
  lost(): boolean;
  /** The pointer or the keys are moving the camera: no 'moveend' yet */
  interacting(): boolean;
  /** Custom layers are drawn: they read the camera of the frame */
  customLayers(): boolean;
  /** The engine with 3D Tiles is loading: not idle */
  upgrading(): boolean;
  /** The frame is drawn: what follows it (the overlays placed, the hover picking asked for at `hovered`) */
  drawn(hovered: Point | undefined): void;
}

/** The frames of a map, asked for and drawn */
export class FrameLoop {
  private readonly host: FrameHost;
  private readonly listeners: MapListeners;
  /** Frame asked for, 0 if none */
  private requested = 0;
  private inFrame = false;
  private animation: Step | undefined;
  /** The pointer's position for the next frame */
  private hoverPoint: Point | undefined;
  private lastCamera = '';
  /** The camera moved since the last 'moveend' */
  private moving = false;
  #settled = false;
  #camera: Camera | undefined;

  /** `listeners`: the map's, told of the frame's events */
  constructor(host: FrameHost, listeners: MapListeners) {
    this.host = host;
    this.listeners = listeners;
  }

  /** The last frame found everything loaded ('idle' sent) */
  get settled(): boolean {
    return this.#settled;
  }

  /** The camera of the frame being drawn: what `getCamera` gives the custom layers while the engine is busy */
  get camera(): Camera | undefined {
    return this.#camera;
  }

  /** Asks for a frame; several calls before the next frame draw it once */
  request(): void {
    // Changes made by listeners during a frame are drawn by that frame; nothing is drawn without a context
    if (this.inFrame || this.host.lost()) return;
    this.requested ||= requestAnimationFrame((now) => this.frame(now));
  }

  /** Runs `step` every frame until it returns false; replaces the running animation */
  animate(step: Step | undefined): void {
    // A flight cut short no longer needs its destination
    if (this.animation) this.host.engine().clearPreload();
    this.animation = step;
    this.request();
  }

  /** The pointer's position for the next frame (undefined: none) */
  hover(point: Point | undefined): void {
    this.hoverPoint = point;
  }

  /** Draws a frame now, in place of the one asked for (a resized canvas is cleared: waiting one frame would flash) */
  drawNow(): void {
    cancelAnimationFrame(this.requested);
    this.frame(performance.now());
  }

  /** Cancels the frame asked for (the context is lost); a new one can be asked for */
  cancel(): void {
    cancelAnimationFrame(this.requested);
    this.requested = 0;
  }

  /** Cancels the frame asked for, for good (the map is destroyed): it stays marked as asked for */
  stop(): void {
    cancelAnimationFrame(this.requested);
  }

  private frame(now: number): void {
    this.requested = 0;
    try {
      this.inFrame = true;
      this.draw(now);
    } finally {
      // Whatever throws in a frame, the next ones are still drawn
      this.inFrame = false;
    }
  }

  private draw(now: number): void {
    if (this.animation && !this.animation(now)) this.animation = undefined;

    const { host, listeners } = this;
    const camera = host.camera();
    const key = `${camera.center}|${camera.zoom}|${camera.pitch}|${camera.bearing}`;
    const moved = key !== this.lastCamera;

    if (moved) {
      // The first frame is no move
      if (!this.moving && this.lastCamera !== '') {
        this.moving = true;
        listeners.emit('movestart', camera);
      }
      this.lastCamera = key;
      listeners.emit('move', camera);
    } else if (this.moving && !this.animation && !host.interacting()) {
      this.moving = false;
      listeners.emit('moveend', camera);
    }

    // Values for the pointer position are computed before drawing, in the same frame
    const hovered = this.hoverPoint;

    if (hovered) {
      this.hoverPoint = undefined;
      // The position under the pointer for its listeners only (a 'hoverfeature' alone does not need it)
      if (listeners.has('hover')) {
        listeners.emit('hover', { point: hovered, lonLat: host.engine().ground(hovered[0], hovered[1]) });
      }
    }

    const t0 = performance.now();

    // The engine asks for the next frame while it draws (uploads past the frame budget, fades): not ignored
    this.inFrame = false;
    // As the engine has it now (a listener may have moved it), for the custom layers drawn in the frame
    this.#camera = host.customLayers() ? host.engine().camera() : undefined;

    const render = (): boolean => {
      try {
        return host.engine().render();
      } finally {
        this.#camera = undefined;
      }
    };
    const loading = render();

    this.inFrame = true;
    host.drawn(hovered);
    listeners.emit('render', { ms: performance.now() - t0 });
    this.inFrame = false;
    // A move goes on until a frame finds the camera still: that frame sends 'moveend'
    if (this.animation || (moved && this.moving)) this.request();
    // Idle: nothing loading (the engine with 3D Tiles neither), no animation, no frame asked for (uploads waiting,
    // fades, listeners)
    if (loading || host.upgrading() || this.animation || this.requested !== 0) {
      this.#settled = false;
    } else if (!this.#settled) {
      this.#settled = true;
      listeners.emit('idle', {});
    }
  }
}
