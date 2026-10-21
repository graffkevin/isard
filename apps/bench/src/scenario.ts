/**
 * The benchmark scenario, the same for every engine: poses in engine-neutral terms (target, distance, tilt, heading),
 * driven frame by frame, with the main-thread frame times, long tasks and loading times measured per phase.
 */

/** Camera looking at a ground point */
export interface Pose {
  lon: number;
  lat: number;
  /** Distance from the camera to the target, metres */
  range: number;
  /** Degrees from the vertical (0: looking straight down) */
  tilt: number;
  /** Degrees clockwise from the north */
  heading: number;
}

/** What the scenario needs from an engine */
export interface BenchEngine {
  readonly name: string;
  setPose(pose: Pose): void;
  /** Nothing left to load or to refine for the current view: the ground (basemap) and the 3D mesh */
  idle(): { ground: boolean; mesh: boolean };
  /** 3D mesh tiles drawn in the last frame */
  meshTiles(): number;
  /** Engine-specific counters, shown with the results */
  info(): string;
}

export interface PhaseResult {
  phase: string;
  durationMs: number;
  frames: number;
  fps: number;
  frameP50: number;
  frameP95: number;
  frameP99: number;
  /** Share of the frames longer than 33 ms (under 30 fps), percent */
  slowFrames: number;
  /** Main-thread tasks over 50 ms, total duration; undefined where the browser does not report them (Firefox) */
  longTaskMs?: number;
  /** Settling phases: time until the ground (basemap), the 3D mesh, both, were complete; undefined if never */
  groundAfterMs?: number;
  meshAfterMs?: number;
  idleAfterMs?: number;
  /** Mesh tiles drawn at the end of the phase, and the time when half of them were drawn */
  meshTilesEnd?: number;
  meshHalfMs?: number;
}

export interface BenchResult {
  engine: string;
  phases: PhaseResult[];
  heapMb?: number;
  info: string;
}

/** Field of view shared by both engines (vertical, degrees) */
export const FOV = 36.87;
/** Screen-space error target of the 3D Tiles, the same for both engines */
export const SSE = 8;
export const LILLE_MESH = 'https://webimaging.lillemetropole.fr/externe/maillage/2020_mel_5cm/tileset.json';
export const ORTHO = {
  url: 'https://data.geopf.fr/wmts',
  layer: 'ORTHOIMAGERY.ORTHOPHOTOS',
  tileMatrixSet: 'PM',
  format: 'image/jpeg',
};

const FRANCE: Pose = { lon: 2.5, lat: 46.6, range: 3_000_000, tilt: 0, heading: 0 };
const LILLE: Pose = { lon: 3.0632, lat: 50.6369, range: 1500, tilt: 60, heading: 20 };
/** Vieux-Lille close up, never seen before in the scenario: the mesh loads from nothing */
const VIEUX_LILLE: Pose = { lon: 3.0598, lat: 50.6435, range: 600, tilt: 55, heading: 200 };

export const START = FRANCE;

/** Interpolated pose: distances geometrically (constant zoom speed), angles linearly */
const between = (a: Pose, b: Pose, t: number): Pose => {
  const k = t < 0.5 ? 4 * t ** 3 : 1 - (-2 * t + 2) ** 3 / 2;
  return {
    lon: a.lon + (b.lon - a.lon) * k,
    lat: a.lat + (b.lat - a.lat) * k,
    range: a.range * (b.range / a.range) ** k,
    tilt: a.tilt + (b.tilt - a.tilt) * k,
    heading: a.heading + (b.heading - a.heading) * k,
  };
};

const frame = () => new Promise<number>((resolve) => requestAnimationFrame(resolve));

/** Long tasks of the main thread, collected while the scenario runs */
const longTasks: { start: number; duration: number }[] = [];
const longTasksReported = PerformanceObserver.supportedEntryTypes.includes('longtask');
if (longTasksReported) {
  new PerformanceObserver((list) => {
    for (const e of list.getEntries()) longTasks.push({ start: e.startTime, duration: e.duration });
  }).observe({ type: 'longtask', buffered: true });
}

const summary = (phase: string, times: number[], start: number, end: number): PhaseResult => {
  const gaps = times.slice(1).map((t, i) => t - (times[i] ?? t));
  const sorted = [...gaps].sort((a, b) => a - b);
  const at = (q: number) => sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))] ?? 0;
  const duration = end - start;
  return {
    phase,
    durationMs: Math.round(duration),
    frames: gaps.length,
    fps: Math.round((gaps.length / duration) * 1000 * 10) / 10,
    frameP50: Math.round(at(0.5) * 10) / 10,
    frameP95: Math.round(at(0.95) * 10) / 10,
    frameP99: Math.round(at(0.99) * 10) / 10,
    slowFrames: gaps.length ? Math.round((gaps.filter((g) => g > 33.4).length / gaps.length) * 100) : 0,
    ...(longTasksReported && {
      longTaskMs: Math.round(
        longTasks.filter((l) => l.start >= start && l.start < end).reduce((s, l) => s + l.duration, 0),
      ),
    }),
  };
};

/** Moves the camera from `from` to `to` over `ms`, one pose per frame */
const animate = async (engine: BenchEngine, phase: string, from: Pose, to: Pose, ms: number): Promise<PhaseResult> => {
  Object.assign(window, { benchPhase: phase });
  const times: number[] = [];
  const start = await frame();
  for (let now = start; now - start < ms; now = await frame()) {
    times.push(now);
    engine.setPose(between(from, to, (now - start) / ms));
  }
  engine.setPose(to);
  return summary(phase, times, start, performance.now());
};

/** Waits until the engine is idle (or `maxMs`), measuring the frames meanwhile; a part counts as complete from the
 * last time it was still busy */
const settle = async (engine: BenchEngine, phase: string, maxMs: number): Promise<PhaseResult> => {
  // The current phase, for inspection from outside (screenshots at given times of a phase)
  Object.assign(window, { benchPhase: phase });
  const times: number[] = [];
  const start = await frame();
  let groundBusy = start;
  let meshBusy = start;
  let done = false;
  const counts: [number, number][] = [];
  for (let now = start; now - start < maxMs; now = await frame()) {
    times.push(now);
    counts.push([now, engine.meshTiles()]);
    const idle = engine.idle();
    if (!idle.ground) groundBusy = now;
    if (!idle.mesh) meshBusy = now;
    // Both idle for 500 ms: done (a pause between two tile batches does not count)
    if (now - Math.max(groundBusy, meshBusy) > 500) {
      done = true;
      break;
    }
  }
  const end = performance.now();
  const result = summary(phase, times, start, end);
  const after = (busy: number) => Math.round(busy === start ? 0 : busy - start);
  if (done || end - groundBusy > 500) result.groundAfterMs = after(groundBusy);
  if (done || end - meshBusy > 500) result.meshAfterMs = after(meshBusy);
  if (done) result.idleAfterMs = after(Math.max(groundBusy, meshBusy));
  const final = counts.at(-1)?.[1] ?? 0;
  result.meshTilesEnd = final;
  const half = counts.find(([, n]) => n >= final / 2 && n > 0);
  if (half && final > 0) result.meshHalfMs = Math.round(half[0] - start);
  return result;
};

export const run = async (engine: BenchEngine): Promise<BenchResult> => {
  const phases: PhaseResult[] = [];
  engine.setPose(FRANCE);
  phases.push(await settle(engine, '1 load France', 30_000));
  phases.push(await animate(engine, '2 flight to Lille', FRANCE, LILLE, 6000));
  phases.push(await settle(engine, '3 settle Lille', 40_000));
  phases.push(await animate(engine, '4 orbit 360°', LILLE, { ...LILLE, heading: LILLE.heading + 360 }, 10_000));
  phases.push(await settle(engine, '5 settle orbit', 20_000));
  phases.push(await animate(engine, '6 back to France', LILLE, FRANCE, 6000));
  // The mesh alone: a jump (no flight) to a spot never seen, nothing of it loaded
  engine.setPose(VIEUX_LILLE);
  phases.push(await settle(engine, '7 jump to Vieux-Lille', 40_000));
  const memory = (performance as Performance & { memory?: { usedJSHeapSize: number } }).memory;
  const result: BenchResult = { engine: engine.name, phases, info: engine.info() };
  if (memory) result.heapMb = Math.round(memory.usedJSHeapSize / 1048576);
  return result;
};

/** Runs the scenario and hands the result to the runner page; `?still=lille` or `?still=france` only holds a pose
 * (to check what an engine shows) */
export const benchmark = async (engine: BenchEngine): Promise<void> => {
  const still = new URLSearchParams(location.search).get('still');
  if (still) {
    engine.setPose(still === 'lille' ? LILLE : FRANCE);
    return;
  }
  const result = await run(engine);
  parent.postMessage({ type: 'bench-result', result }, '*');
  (window as Window & { benchResult?: BenchResult }).benchResult = result;
};
