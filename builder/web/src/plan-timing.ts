// The timing page of the ground plan: builds the prepared requests of a `requests` folder in the plan
// worker, in any browser, and compares the times and digests with the native ones. The dev server
// serves it; no build ships it.
//
//   plan-timing.html?requests=/@fs/<repository>/.artifacts/ground-plan/requests/&runs=5&cells=34296-23098
import type { Failed, PlanRequest, Timed } from './lib/trailview/plan/plan-worker';

interface Entry { cell: [number, number]; file: string; places: string[]; request: string; digest: string; native: Record<string, number> }
interface Row { cell: string; places: string[]; faces?: number; edges?: number; junctions: number; cold: number; median: Record<string, number>; p95: Record<string, number>; native: number; ratio: number; heap: number; same: boolean; error?: string }

const WASM = new URL('./lib/trailview/plan/pkg/obc_ground_plan_bridge_bg.wasm', import.meta.url);
const q = new URLSearchParams(location.search);
const folder = q.get('requests'), runs = Math.min(50, Math.max(1, Number(q.get('runs') ?? 5) || 5)), only = q.get('cells')?.split(',');
const about = document.querySelector<HTMLElement>('#about')!, table = document.querySelector<HTMLTableElement>('#cells')!, summary = document.querySelector<HTMLElement>('#summary')!;
const copy = document.querySelector<HTMLButtonElement>('#copy')!;

const quantile = (values: number[], at: number) => {
    const sorted = [...values].sort((a, b) => a - b);
    return sorted[Math.min(sorted.length - 1, Math.floor(at * sorted.length))] ?? NaN;
};
const ms = (value: number) => (Number.isFinite(value) ? value.toFixed(1) : '–');

async function main(): Promise<void> {
    if (!folder) throw new Error('Give the folder of a `requests` run: ?requests=/@fs/<repository>/.artifacts/ground-plan/requests/');
    const base = new URL(folder.endsWith('/') ? folder : `${folder}/`, location.href);
    const index: Entry[] = await (await fetch(new URL('index.json', base))).json();
    const entries = index.filter(({ cell }) => !only || only.includes(cell.join('-')));
    const module = await WebAssembly.compile(await (await fetch(WASM)).arrayBuffer());
    const timing = performance.getEntriesByName(WASM.href)[0] as PerformanceResourceTiming | undefined;
    const worker = new Worker(new URL('./lib/trailview/plan/plan-worker.ts', import.meta.url), { type: 'module' });
    const next = <T>() => new Promise<T>(resolve => (worker.onmessage = ({ data }) => resolve(data)));
    worker.postMessage({ type: 'start', module } satisfies PlanRequest);
    const started = await next<{ type: 'ready'; error?: string }>();
    if (started.error) throw new Error(`The plan worker did not start: ${started.error}`);
    const head = {
        userAgent: navigator.userAgent, cores: navigator.hardwareConcurrency, runs,
        wasm: { bytes: timing?.decodedBodySize || undefined, transferred: timing?.encodedBodySize || undefined },
    };
    about.textContent = `${head.userAgent}\ncores ${head.cores} · runs ${runs} · WASM ${head.wasm.bytes ? `${(head.wasm.bytes / 1024).toFixed(0)} KiB` : 'size unknown'}${head.wasm.transferred ? ` (${(head.wasm.transferred / 1024).toFixed(0)} KiB sent)` : ''} · ${entries.length} cells`;
    let stages: string[] = [];
    const rows: Row[] = [];
    for (const [id, entry] of entries.entries()) {
        const request = await (await fetch(new URL(entry.file, base))).arrayBuffer();
        worker.postMessage({ type: 'time', id, request, runs } satisfies PlanRequest, [request]);
        const result = await next<Timed | Failed>(), cell = entry.cell.join('-');
        if (!result.ok) {
            rows.push({ cell, places: entry.places, junctions: 0, cold: NaN, median: {}, p95: {}, native: entry.native.total, ratio: NaN, heap: 0, same: false, error: result.error });
        } else {
            stages = [...Object.keys(entry.native).filter(stage => stage !== 'total'), 'total'];
            const per = (i: number) => result.runs.map(times => times[i]);
            const median = Object.fromEntries(stages.map((stage, i) => [stage, quantile(per(i), 0.5)]));
            const p95 = Object.fromEntries(stages.map((stage, i) => [stage, quantile(per(i), 0.95)]));
            const stats = JSON.parse(result.stats) as { graph: { junctions: number }; faces?: { faces: number; edges: number } };
            rows.push({
                cell, places: entry.places, faces: stats.faces?.faces, edges: stats.faces?.edges, junctions: stats.graph.junctions,
                cold: result.cold.at(-1)!, median, p95, native: entry.native.total, ratio: median.total / entry.native.total, heap: result.heap, same: result.digest === entry.digest,
            });
        }
        draw(rows, stages);
    }
    worker.terminate();
    const ok = rows.filter(row => !row.error), totals = ok.map(row => row.median.total);
    const result = {
        ...head, cells: rows,
        summary: { median: quantile(totals, 0.5), p95: quantile(totals, 0.95), heap: Math.max(0, ...ok.map(row => row.heap)), same: rows.filter(row => row.same).length, of: rows.length },
        done: true,
    };
    summary.textContent = `over ${ok.length} cells: median ${ms(result.summary.median)} ms · p95 ${ms(result.summary.p95)} ms of the per-cell medians · largest heap ${(result.summary.heap / 2 ** 20).toFixed(0)} MiB · ${result.summary.same} of ${result.summary.of} digests equal the native ones`;
    Object.assign(window, { planTiming: result });
    copy.disabled = false;
    copy.onclick = () => void navigator.clipboard.writeText(JSON.stringify(result, null, 1));
}

function draw(rows: Row[], stages: string[]): void {
    const cells = (values: string[], tag = 'td') => values.map(value => `<${tag}>${value}</${tag}>`).join('');
    table.innerHTML = `<tr>${cells(['cell', 'places', 'faces', 'edges', 'junctions', 'cold', ...stages.flatMap(s => [`${s} median`, `${s} p95`]), 'native', 'ratio', 'heap MiB', 'digest'], 'th')}</tr>`
        + rows.map(row => `<tr>${cells([
            row.cell, row.places.join(' '), String(row.faces ?? '–'), String(row.edges ?? '–'), String(row.junctions), ms(row.cold),
            ...stages.flatMap(s => [ms(row.median[s]), ms(row.p95[s])]), ms(row.native), Number.isFinite(row.ratio) ? row.ratio.toFixed(2) : '–', (row.heap / 2 ** 20).toFixed(0),
            row.error ? `<span class="bad">${row.error}</span>` : row.same ? 'equal' : '<span class="bad">differs</span>',
        ])}</tr>`).join('');
}

main().catch((error: unknown) => {
    about.textContent = error instanceof Error ? error.message : String(error);
    Object.assign(window, { planTiming: { error: about.textContent, done: true } });
});
