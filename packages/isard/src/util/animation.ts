/**
 * Camera animations: each is a step run every frame by the map (`GeoMap.animate`) until it returns false, setting the
 * camera of the engine in use at that frame (it may be replaced during a flight).
 */

import type { AnimationOptions, FlyToOptions } from '../types.ts';
import type { Camera, Engine } from '../wasm/isard.js';
import { fromMercator, toMercator } from './geo.ts';

/** A step of an animation, at `now` (milliseconds, the frame's time): false once done */
export type Step = (now: number) => boolean;

/** What a step needs of the engine, read at each frame */
type Target = () => Pick<Engine, 'setCamera' | 'clearPreload'>;

/** MapLibre's default easing: cubic Bézier (0.25, 0.1, 0.25, 1) */
export const defaultEasing = (t: number): number => {
  if (t <= 0 || t >= 1) return Math.max(0, Math.min(1, t));

  // Solve x(u) = t for the curve parameter, Newton then bisection
  const [x1, y1, x2, y2] = [0.25, 0.1, 0.25, 1];
  const bezier = (u: number, a: number, b: number) => 3 * a * u * (1 - u) ** 2 + 3 * b * u * u * (1 - u) + u ** 3;
  let u = t;

  for (let i = 0; i < 8; i += 1) {
    const x = bezier(u, x1, x2) - t;
    const dx = 3 * x1 * (1 - u) ** 2 + 6 * (x2 - x1) * u * (1 - u) + 3 * (1 - x2) * u * u;

    if (Math.abs(x) < 1e-6 || dx === 0) break;
    u = Math.max(0, Math.min(1, u - x / dx));
  }

  return bezier(u, y1, y2);
};

/** The step of an animated move from `from` to `to`: over `duration` milliseconds (or the options), ease-out by
 * default, the bearing turning the shortest way round */
export const easeTo = (engine: Target, from: Camera, to: Camera, options: number | AnimationOptions = 600): Step => {
  const { duration = 600, easing = (t: number) => 1 - (1 - t) ** 3 } =
    typeof options === 'number' ? { duration: options } : options;
  // Shortest way round
  const turn = ((to.bearing - from.bearing + 540) % 360) - 180;
  const start = performance.now();

  return (now) => {
    if (now - start >= duration) engine().clearPreload();

    const t = duration > 0 ? Math.min(1, (now - start) / duration) : 1;
    const k = easing(t);
    const mix = (a: number, b: number) => a + (b - a) * k;

    engine().setCamera(
      mix(from.center[0], to.center[0]),
      mix(from.center[1], to.center[1]),
      mix(from.zoom, to.zoom),
      mix(from.pitch, to.pitch),
      from.bearing + turn * k,
    );

    return t < 1;
  };
};

/**
 * The step of a flight from `from` to `to` that zooms out on its way and back in (van Wijk and Nuij's optimal path, as
 * MapLibre's `flyTo`): long moves stay readable. Pitch and bearing follow the progress. `viewport`: the larger side of
 * the canvas, CSS pixels.
 */
export const flyTo = (engine: Target, from: Camera, to: Camera, viewport: number, options: FlyToOptions = {}): Step => {
  const { curve: rho = 1.42, speed = 1.2, easing = defaultEasing } = options;
  const [x0, y0] = toMercator(from.center);
  const [x1, y1] = toMercator(to.center);
  // Across the antimeridian, the short way
  const dx = x1 - x0 - Math.round(x1 - x0);
  const dy = y1 - y0;
  // In pixels at the start zoom (512 px world at zoom 0)
  const world = 512 * 2 ** from.zoom;
  const u1 = Math.hypot(dx, dy) * world;
  const w0 = Math.max(viewport, 1);
  const w1 = w0 / 2 ** (to.zoom - from.zoom);
  const rho2 = rho * rho;
  const r = (i: 0 | 1) => {
    const b = (w1 * w1 - w0 * w0 + (i ? -1 : 1) * rho2 * rho2 * u1 * u1) / (2 * (i ? w1 : w0) * rho2 * u1);

    return Math.log(Math.sqrt(b * b + 1) - b);
  };
  // No move on the ground (or nearly): a plain zoom
  const still = u1 < 1e-6 || !Number.isFinite(r(0)) || !Number.isFinite(r(1));
  const r0 = still ? 0 : r(0);
  const length = still ? Math.abs(Math.log(w1 / w0)) / rho : (r(1) - r0) / rho;
  const width = (s: number) =>
    still ? Math.exp((w1 < w0 ? -1 : 1) * rho * s) : Math.cosh(r0) / Math.cosh(r0 + rho * s);
  const along = (s: number) =>
    still ? 0 : (w0 * ((Math.cosh(r0) * Math.tanh(r0 + rho * s) - Math.sinh(r0)) / rho2)) / u1;
  const duration = options.duration ?? (1000 * length) / speed;
  const turn = ((to.bearing - from.bearing + 540) % 360) - 180;
  const start = performance.now();

  return (now) => {
    if (now - start >= duration) engine().clearPreload();

    const t = duration > 0 ? Math.min(1, (now - start) / duration) : 1;
    const k = easing(t);
    const s = k * length;
    const u = t >= 1 ? 1 : along(s);
    const zoom = t >= 1 ? to.zoom : from.zoom + Math.log2(1 / width(s));
    const [lon, lat] = fromMercator([x0 + dx * u, y0 + dy * u]);

    engine().setCamera(lon, lat, zoom, from.pitch + (to.pitch - from.pitch) * k, from.bearing + turn * k);

    return t < 1;
  };
};
