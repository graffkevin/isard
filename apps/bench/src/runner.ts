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

/** Metrics shown per phase, and whether higher is better */
const METRICS: [keyof PhaseResult, string, boolean][] = [
  ['fps', 'fps', true],
  ['frameP50', 'frame p50 ms', false],
  ['frameP95', 'frame p95 ms', false],
  ['frameP99', 'frame p99 ms', false],
  ['slowFrames', '% frames > 33 ms', false],
  ['longTaskMs', 'long tasks ms', false],
  ['idleAfterMs', 'complete after ms', false],
];

function table(results: BenchResult[]): string {
  const phases = results[0]?.phases.map((p) => p.phase) ?? [];
  const head = `<tr><th>phase / metric</th>${results.map((r) => `<th>${r.engine}</th>`).join('')}</tr>`;
  const rows: string[] = [];
  for (const phase of phases) {
    for (const [key, label, higher] of METRICS) {
      const values = results.map((r) => r.phases.find((p) => p.phase === phase)?.[key]);
      if (values.every((v) => v === undefined)) continue;
      const numbers = values.filter((v): v is number => typeof v === 'number');
      const best = higher ? Math.max(...numbers) : Math.min(...numbers);
      const cells = values.map(
        (v) => `<td class="${v === best && numbers.length > 1 ? 'best' : ''}">${v ?? 'never'}</td>`,
      );
      rows.push(`<tr><td>${phase}: ${label}</td>${cells.join('')}</tr>`);
    }
  }
  rows.push(`<tr><td>JS heap MB</td>${results.map((r) => `<td>${r.heapMb ?? '-'}</td>`).join('')}</tr>`);
  rows.push(`<tr><td>engine info</td>${results.map((r) => `<td>${r.info}</td>`).join('')}</tr>`);
  return `<table>${head}${rows.join('')}</table>`;
}

button.onclick = async () => {
  button.disabled = true;
  const results: BenchResult[] = [];
  for (const engine of ENGINES) {
    status.textContent = `running ${engine}…`;
    results.push(await once(engine));
    output.innerHTML = table(results);
  }
  frame.src = 'about:blank';
  status.textContent = 'done';
  button.disabled = false;
  (window as Window & { benchResults?: BenchResult[] }).benchResults = results;
};
