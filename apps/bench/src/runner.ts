/** Runner page: plays the scenario with each engine in turn, then compares them phase by phase */
import type { BenchResult, PhaseResult } from './scenario.ts';

const ENGINES = ['geo-render', 'itowns', 'tiles-renderer'] as const;

function element<T extends HTMLElement>(id: string, type: new () => T): T {
  const found = document.getElementById(id);
  if (!(found instanceof type)) throw new Error(`#${id} missing`);
  return found;
}

const button = element('run', HTMLButtonElement);
const status = element('status', HTMLSpanElement);
const frame = element('frame', HTMLIFrameElement);
const output = element('results', HTMLDivElement);

function once(page: string): Promise<BenchResult> {
  return new Promise((resolve) => {
    const listener = (event: MessageEvent<{ type?: string; result?: BenchResult }>) => {
      if (event.data.type === 'bench-result' && event.data.result) {
        removeEventListener('message', listener);
        resolve(event.data.result);
      }
    };
    addEventListener('message', listener);
    frame.src = `./${page}.html?run=${Date.now()}`;
  });
}

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

function phase(result: BenchResult, name: string): PhaseResult | undefined {
  return result.phases.find((p) => p.phase.startsWith(name));
}

/** Frame-weighted average of a metric over the motion phases */
function motion(result: BenchResult, key: 'fps' | 'slowFrames'): number | undefined {
  const phases = result.phases.filter((p) => MOTION.includes(p.phase));
  const frames = phases.reduce((n, p) => n + p.frames, 0);
  if (!frames) return undefined;
  if (key === 'fps') {
    const ms = phases.reduce((n, p) => n + p.durationMs, 0);
    return Math.round((frames / ms) * 1000);
  }
  return Math.round(phases.reduce((n, p) => n + p.slowFrames * p.frames, 0) / frames);
}

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

function seconds(ms: number | undefined): number | undefined {
  return ms === undefined ? undefined : Math.round(ms / 100) / 10;
}

/** Cells of a row, the best one highlighted; engines not run yet show a placeholder */
function cells(values: (number | undefined | null)[], higher: boolean, missing = 'never'): string {
  const numbers = values.filter((v): v is number => typeof v === 'number');
  const best = higher ? Math.max(...numbers) : Math.min(...numbers);
  return values
    .map((v) => {
      if (v === null) return '<td class="pending">…</td>';
      return `<td class="${v === best && numbers.length > 1 ? 'best' : ''}">${v ?? missing}</td>`;
    })
    .join('');
}

/** Engine columns in a fixed order, null while an engine has not run in this round */
function columns(results: BenchResult[]): (BenchResult | null)[] {
  return NAMES.map((name) => results.find((r) => r.engine.startsWith(name)) ?? null);
}

const NAMES = ['geo-render', 'iTowns', '3DTilesRendererJS'];

function summaryTable(results: BenchResult[]): string {
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
}

function detailTable(results: BenchResult[]): string {
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
}

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
  frame.src = 'about:blank';
  status.textContent = 'done';
  button.disabled = false;
  (window as Window & { benchResults?: BenchResult[][] }).benchResults = rounds.map((r) => r.results);
};

// The tables are there from the start, filled in as the engines run
output.innerHTML = ['Round 2, warm caches (reference)', 'Round 1, cold caches (first visit)']
  .map((title) => `<h2>${title}</h2>${summaryTable([])}`)
  .join('');
