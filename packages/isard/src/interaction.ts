/**
 * Mouse and touch controls. Every gesture only updates the camera and asks for a frame: the engine draws at most
 * once per display refresh, whatever the event rate of the device.
 */
import type { GeoMap } from './map.ts';
import type { Point } from './types.ts';

/** Zoom levels per wheel pixel (a mouse notch is about 100 px: 1/3 of a level) */
const WHEEL_RATE = 1 / 300;
/** Share of the remaining wheel zoom applied per frame (smooth, about 150 ms to settle) */
const WHEEL_EASE = 0.3;
/** Degrees per pixel when rotating or tilting with the mouse */
const ROTATE_RATE = 0.4;
/** A press that moves less than this (CSS pixels) is a click */
const CLICK_TOLERANCE = 4;
/** Pan speed kept on release, decaying with this time constant (ms) */
const INERTIA_DECAY = 280;

interface Pointer {
  point: Point;
  start: Point;
  button: number;
  rotate: boolean;
}

export class Interaction {
  private readonly pointers = new Map<number, Pointer>();
  private readonly cleanup: (() => void)[] = [];
  private wheel = { pending: 0, at: [0, 0] as Point };
  /** Recent pan moves, for the release speed: [time, dx, dy] */
  private trail: [number, number, number][] = [];
  private moved = false;

  constructor(private readonly map: GeoMap) {
    const canvas = map.canvas;
    const listen = <K extends keyof HTMLElementEventMap>(
      type: K,
      handler: (e: HTMLElementEventMap[K]) => void,
      options?: AddEventListenerOptions,
    ) => {
      canvas.addEventListener(type, handler, options);
      this.cleanup.push(() => canvas.removeEventListener(type, handler));
    };
    listen('pointerdown', (e) => this.down(e));
    listen('pointermove', (e) => this.move(e));
    listen('pointerup', (e) => this.up(e));
    listen('pointercancel', (e) => this.up(e));
    listen('pointerleave', (e) => {
      if (e.pointerType !== 'mouse') return;
      map.hover(undefined);
      map.emit('leave', {});
    });
    // On the container, the wheel zooms over markers and popups too
    const container = map.getContainer();
    const wheel = (e: WheelEvent) => this.onWheel(e);
    container.addEventListener('wheel', wheel, { passive: false });
    this.cleanup.push(() => container.removeEventListener('wheel', wheel));
    listen('dblclick', (e) => this.zoomBy(e.shiftKey ? -1 : 1, this.local(e)));
    listen('contextmenu', (e) => e.preventDefault());
  }

  destroy(): void {
    for (const off of this.cleanup) off();
  }

  /** A gesture holds the map (pointers down): the camera has not stopped moving */
  get active(): boolean {
    return this.pointers.size > 0;
  }

  private local(e: MouseEvent): Point {
    const r = this.map.canvas.getBoundingClientRect();
    return [e.clientX - r.left, e.clientY - r.top];
  }

  private down(e: PointerEvent): void {
    this.map.canvas.setPointerCapture(e.pointerId);
    const point = this.local(e);
    const rotate = e.button === 2 || e.ctrlKey || e.shiftKey;
    this.pointers.set(e.pointerId, { point, start: point, button: e.button, rotate });
    this.trail = [];
    this.moved = false;
    // Grabbing the map stops any running animation (inertia, wheel zoom, easeTo)
    this.wheel.pending = 0;
    this.map.animate(undefined);
  }

  private move(e: PointerEvent): void {
    const point = this.local(e);
    const p = this.pointers.get(e.pointerId);
    if (!p) {
      if (e.pointerType === 'mouse') this.map.hover(point);
      return;
    }
    const [dx, dy] = [point[0] - p.point[0], point[1] - p.point[1]];
    if (Math.hypot(point[0] - p.start[0], point[1] - p.start[1]) > CLICK_TOLERANCE) this.moved = true;
    const engine = this.map.engine;
    const other = [...this.pointers.values()].find((q) => q !== p);
    if (this.pointers.size === 2 && other) {
      // Pinch: zoom by the change of spread, turn by the change of angle, pan by the midpoint
      const before = Math.hypot(p.point[0] - other.point[0], p.point[1] - other.point[1]);
      const after = Math.hypot(point[0] - other.point[0], point[1] - other.point[1]);
      const angle = (q: Point) => Math.atan2(q[1] - other.point[1], q[0] - other.point[0]);
      const mid = (q: Point): Point => [(q[0] + other.point[0]) / 2, (q[1] + other.point[1]) / 2];
      engine.pan(...mid(p.point), ...mid(point));
      if (before > 0 && after > 0) engine.zoomAt(Math.log2(after / before), ...mid(point));
      engine.rotate((-(angle(point) - angle(p.point)) * 180) / Math.PI, 0);
    } else if (p.rotate) {
      engine.rotate(-dx * ROTATE_RATE, -dy * ROTATE_RATE);
    } else {
      engine.pan(p.point[0], p.point[1], point[0], point[1]);
      this.trail.push([e.timeStamp, dx, dy]);
    }
    p.point = point;
    this.map.requestRender();
  }

  private up(e: PointerEvent): void {
    const p = this.pointers.get(e.pointerId);
    if (!p) return;
    this.pointers.delete(e.pointerId);
    // The next frame sends 'moveend' if the camera stays
    this.map.requestRender();
    if (!this.moved && e.type === 'pointerup' && p.button === 0 && this.pointers.size === 0) {
      this.map.emit('click', { point: p.point, lonLat: this.map.unproject(p.point) });
      return;
    }
    if (!p.rotate && this.pointers.size === 0) this.inertia(e.timeStamp, p.point);
  }

  /** Keeps panning at the release speed (last 50 ms), decaying exponentially */
  private inertia(now: number, from: Point): void {
    const recent = this.trail.filter(([t]) => now - t < 50);
    const first = recent[0];
    if (!first || recent.length < 2) return;
    const span = Math.max(16, now - first[0]);
    let vx = recent.reduce((s, [, dx]) => s + dx, 0) / span;
    let vy = recent.reduce((s, [, , dy]) => s + dy, 0) / span;
    if (Math.hypot(vx, vy) < 0.1) return;
    let last = performance.now();
    let point = from;
    this.map.animate((t) => {
      const dt = Math.min(64, t - last);
      last = t;
      const decay = Math.exp(-dt / INERTIA_DECAY);
      const next: Point = [point[0] + vx * dt, point[1] + vy * dt];
      this.map.engine.pan(point[0], point[1], next[0], next[1]);
      point = next;
      vx *= decay;
      vy *= decay;
      return Math.hypot(vx, vy) > 0.02;
    });
  }

  private onWheel(e: WheelEvent): void {
    e.preventDefault();
    // Lines (Firefox mouse wheels) are about 33 px
    const pixels = e.deltaMode === 1 ? e.deltaY * 33 : e.deltaY;
    this.zoomBy(
      Math.max(-1, Math.min(1, -pixels * WHEEL_RATE)),
      this.local(e),
      e.deltaMode === 0 && Math.abs(pixels) < 40,
    );
  }

  /** Smooth zoom around a screen point; touchpads (`direct`) follow the fingers without easing */
  private zoomBy(delta: number, at: Point, direct = false): void {
    if (direct) {
      this.map.engine.zoomAt(delta, at[0], at[1]);
      this.map.requestRender();
      return;
    }
    // Accumulate: a second notch before the end of the first one adds to it
    this.wheel.pending += delta;
    this.wheel.at = at;
    this.map.animate(() => {
      const step = Math.abs(this.wheel.pending) < 0.002 ? this.wheel.pending : this.wheel.pending * WHEEL_EASE;
      this.wheel.pending -= step;
      this.map.engine.zoomAt(step, this.wheel.at[0], this.wheel.at[1]);
      return this.wheel.pending !== 0;
    });
  }
}
