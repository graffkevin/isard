/** Runner page: plays the scenario with each engine in turn, then compares them phase by phase */
import { type BenchResult, LILLE_MESH, type PhaseResult } from './scenario.ts';

const ENGINES = ['isard', 'itowns', 'tiles-renderer'] as const;
/** Column names, in the order of `ENGINES` (a result's engine name starts with one) */
const NAMES = ['isard', 'iTowns', '3DTilesRendererJS'];

const element = <T extends HTMLElement>(id: string, type: new () => T): T => {
  const found = document.getElementById(id);
  if (!(found instanceof type)) throw new Error(`#${id} missing`);
  return found;
};

const button = element('run', HTMLButtonElement);
const status = element('status', HTMLSpanElement);
let frame = element('frame', HTMLIFrameElement);
const output = element('results', HTMLDivElement);

/** An engine stuck this long (a crashed page, a lost GPU) is marked as timed out instead of blocking the run */
const ENGINE_TIMEOUT_MS = 4 * 60_000;

/**
 * Frees the GPU of the engine that just ran, then puts a fresh frame in place: browsers (Firefox notably) keep the
 * WebGL context of a navigated-away page until garbage collection, so the engines would pile up on the GPU
 */
const freshFrame = async (): Promise<void> => {
  try {
    for (const canvas of frame.contentDocument?.querySelectorAll('canvas') ?? []) {
      const gl = canvas.getContext('webgl2') ?? canvas.getContext('webgl');
      gl?.getExtension('WEBGL_lose_context')?.loseContext();
    }
  } catch {
    // Nothing to free (an empty or cross-origin frame)
  }
  const next = document.createElement('iframe');
  next.id = frame.id;
  next.title = frame.title;
  frame.replaceWith(next);
  frame = next;
  // A moment for the browser to release the old page
  await new Promise((resolve) => setTimeout(resolve, 1000));
};

/**
 * Opens the connection to the mesh server before an engine starts: through the IGN proxy, the first tunnel to it after
 * a while takes ~10 s (its name resolves slowly), and Firefox holds the page's other requests back meanwhile; without
 * this, whichever engine came first after the proxy forgot the name paid for it
 */
const warmUp = async (): Promise<void> => {
  try {
    await fetch(new URL('Data/L11_32.b3dm', LILLE_MESH), { mode: 'no-cors', cache: 'no-store' });
  } catch {
    // The engines will meet the same error
  }
};

const once = async (page: string): Promise<BenchResult> => {
  await freshFrame();
  await warmUp();
  return new Promise((resolve) => {
    const finish = (result: BenchResult) => {
      clearTimeout(timer);
      removeEventListener('message', listener);
      resolve(result);
    };
    const listener = (event: MessageEvent<{ type?: string; result?: BenchResult }>) => {
      if (event.data.type === 'bench-result' && event.data.result) finish(event.data.result);
    };
    const name = NAMES[ENGINES.indexOf(page as (typeof ENGINES)[number])] ?? page;
    const timer = setTimeout(() => finish({ engine: `${name} (timed out)`, phases: [], info: '' }), ENGINE_TIMEOUT_MS);
    addEventListener('message', listener);
    frame.src = `./${page}.html?run=${Date.now()}`;
  });
};

/** Detailed metrics per phase, and whether higher is better */
const METRICS: [keyof PhaseResult, string, boolean][] = [
  ['fps', 'fps', true],
  ['frameP95', 'frame p95 ms', false],
  ['slowFrames', '% frames > 33 ms', false],
  ['longTaskMs', 'long tasks ms', false],
  ['groundAfterMs', 'basemap complete ms', false],
  ['meshAfterMs', '3D mesh complete ms', false],
  ['meshHalfMs', '3D mesh half shown ms', false],
  ['meshTilesEnd', '3D mesh tiles shown', true],
];

const MOTION = ['2 flight to Lille', '4 orbit 360°', '6 back to France'];

const phase = (result: BenchResult, name: string): PhaseResult | undefined => {
  return result.phases.find((p) => p.phase.startsWith(name));
};

/** Frame-weighted average of a metric over the motion phases */
const motion = (result: BenchResult, key: 'fps' | 'slowFrames'): number | undefined => {
  const phases = result.phases.filter((p) => MOTION.includes(p.phase));
  const frames = phases.reduce((n, p) => n + p.frames, 0);
  if (!frames) return undefined;
  if (key === 'fps') {
    const ms = phases.reduce((n, p) => n + p.durationMs, 0);
    return Math.round((frames / ms) * 1000);
  }
  return Math.round(phases.reduce((n, p) => n + p.slowFrames * p.frames, 0) / frames);
};

/** The few numbers that tell the story, in plain words */
const SUMMARY: {
  label: string;
  unit: string;
  higher: boolean;
  value: (r: BenchResult) => number | undefined;
  /** Shown when there is no value: by default the event never happened, here the browser does not measure it */
  missing?: string;
}[] = [
  { label: 'Smoothness while moving', unit: 'fps', higher: true, value: (r) => motion(r, 'fps') },
  { label: 'Stutter while moving', unit: '% slow frames', higher: false, value: (r) => motion(r, 'slowFrames') },
  {
    label: 'Main thread blocked',
    unit: 'ms',
    higher: false,
    value: (r) =>
      r.phases.every((p) => p.longTaskMs !== undefined)
        ? r.phases.reduce((n, p) => n + (p.longTaskMs ?? 0), 0)
        : undefined,
    missing: 'n/a (Chrome only)',
  },
  { label: 'France basemap shown', unit: 's', higher: false, value: (r) => seconds(phase(r, '1')?.groundAfterMs) },
  {
    label: 'Lille complete after the flight',
    unit: 's',
    higher: false,
    value: (r) => seconds(phase(r, '3')?.idleAfterMs),
  },
  { label: '3D mesh half shown (jump)', unit: 's', higher: false, value: (r) => seconds(phase(r, '7')?.meshHalfMs) },
  { label: '3D mesh complete (jump)', unit: 's', higher: false, value: (r) => seconds(phase(r, '7')?.meshAfterMs) },
  {
    label: 'JavaScript memory',
    unit: 'MB',
    higher: false,
    value: (r) => r.heapMb,
    missing: 'n/a (Chrome only)',
  },
];

const seconds = (ms: number | undefined): number | undefined => {
  return ms === undefined ? undefined : Math.round(ms / 100) / 10;
};

/** Cells of a row, the best one highlighted; engines not run yet show a placeholder */
const cells = (values: (number | undefined | null)[], higher: boolean, missing = 'never'): string => {
  const numbers = values.filter((v): v is number => typeof v === 'number');
  const best = higher ? Math.max(...numbers) : Math.min(...numbers);
  return values
    .map((v) => {
      if (v === null) return '<td class="pending">…</td>';
      return `<td class="${v === best && numbers.length > 1 ? 'best' : ''}">${v ?? missing}</td>`;
    })
    .join('');
};

/** Engine columns in a fixed order, null while an engine has not run in this round */
const columns = (results: BenchResult[]): (BenchResult | null)[] => {
  return NAMES.map((name) => results.find((r) => r.engine.startsWith(name)) ?? null);
};

const summaryTable = (results: BenchResult[]): string => {
  const cols = columns(results);
  const head = `<tr><th></th>${cols.map((r, i) => `<th>${r?.engine ?? NAMES[i]}</th>`).join('')}</tr>`;
  const rows = SUMMARY.map(
    (m) =>
      `<tr><td>${m.label} <span class="unit">${m.unit}</span></td>${cells(
        cols.map((r) => (r ? m.value(r) : null)),
        m.higher,
        m.missing,
      )}</tr>`,
  );
  return `<table class="summary">${head}${rows.join('')}</table>`;
};

const detailTable = (results: BenchResult[]): string => {
  const head = `<tr><th>phase / metric</th>${results.map((r) => `<th>${r.engine}</th>`).join('')}</tr>`;
  const rows: string[] = [];
  for (const name of results[0]?.phases.map((p) => p.phase) ?? []) {
    for (const [key, label, higher] of METRICS) {
      const values = results.map((r) => r.phases.find((p) => p.phase === name)?.[key]);
      if (values.every((v) => v === undefined)) continue;
      rows.push(
        `<tr><td>${name}: ${label}</td>${cells(
          values.map((v) => (typeof v === 'number' ? v : undefined)),
          higher,
        )}</tr>`,
      );
    }
  }
  return `<table>${head}${rows.join('')}</table>`;
};

/**
 * Two rounds: the first one warms the servers (the Lille mesh server takes ~15 s per cold file) and the browser cache,
 * which would otherwise penalize whichever engine runs first; the second one, in reverse order, is the reference.
 */
button.onclick = async () => {
  button.disabled = true;
  const rounds: { title: string; order: readonly string[]; results: BenchResult[] }[] = [
    { title: 'Round 2, warm caches (reference)', order: [...ENGINES].reverse(), results: [] },
    { title: 'Round 1, cold caches (first visit)', order: ENGINES, results: [] },
  ];
  const render = () => {
    output.innerHTML = rounds
      .map((r) => {
        const details =
          r.results.length > 0
            ? `<details><summary>Details per phase</summary>${detailTable(r.results)}</details>`
            : '';
        return `<h2>${r.title}</h2>${summaryTable(r.results)}${details}`;
      })
      .join('');
  };
  render();
  for (const round of [...rounds].reverse()) {
    for (const engine of round.order) {
      status.textContent = `${round.title}: running ${engine}…`;
      round.results.push(await once(engine));
      render();
    }
  }
  await freshFrame();
  status.textContent = 'done';
  button.disabled = false;
  (window as Window & { benchResults?: BenchResult[][] }).benchResults = rounds.map((r) => r.results);
};

// The tables are there from the start, filled in as the engines run
output.innerHTML = ['Round 2, warm caches (reference)', 'Round 1, cold caches (first visit)']
  .map((title) => `<h2>${title}</h2>${summaryTable([])}`)
  .join('');
