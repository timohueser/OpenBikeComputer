/** Browser benchmark against a local planner preview and its real terrain. */
import { readdir } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { createRequire } from 'node:module';
const require = createRequire(new URL('../builder/app/tests/browser/package.json', import.meta.url));
const { chromium } = require('playwright');
const base = process.argv[2] ?? 'http://127.0.0.1:4190';
const browser = await chromium.launch({ headless: true });
try {
    const page = await browser.newPage();
    await page.goto(`${base}/planner.html`);
    await page.waitForLoadState('networkidle');
    const assets = fileURLToPath(new URL('../builder/app/dist/planner/assets/', import.meta.url));
    const bundle = (await readdir(assets)).find(name => /^sun-worker-.*\.js$/.test(name));
    if (!bundle) throw new Error('Build the planner before benchmarking');
    const result = await page.evaluate(async workerUrl => {
        const { config } = await import('/src/lib/planner/map-data.ts');
        const worker = new Worker(workerUrl, { type: 'module' });
        let serial = 0;
        const jobs = new Map();
        const metrics = { stats: null };
        worker.onmessage = ({ data }) => {
            const job = jobs.get(data.id); if (!job) return;
            jobs.delete(data.id); metrics.stats = data.stats ?? metrics.stats;
            if (data.error) job.reject(new Error(data.error)); else job.resolve(data.result);
        };
        const request = (payload, signal) => new Promise((resolve, reject) => {
            const id = ++serial;
            const cancel = () => { worker.postMessage({ cancel: id }); jobs.delete(id); reject(signal.reason); };
            signal.addEventListener('abort', cancel, { once: true });
            jobs.set(id, { resolve: value => { signal.removeEventListener('abort', cancel); resolve(value); }, reject });
            worker.postMessage({ id, url: config.layers.sun, dem: config.terrain, ...payload });
        });
        const client = {
            metrics,
            tile: (z, x, y, date, minute, signal) => request({ kind: 'tile', z, x, y, date, minute, size: 128 }, signal),
            day: (coordinate, date, signal) => request({ kind: 'day', coordinate, date }, signal),
            sample: (coordinate, date, minute, signal) => request({ kind: 'sample', coordinates: [coordinate], date, minute }, signal),
            dispose: () => worker.terminate(),
        };
        const rows = [], checks = [];
        const tile = (lon, lat, z) => [Math.floor((lon + 180) / 360 * 2 ** z), Math.floor((1 - Math.asinh(Math.tan(lat * Math.PI / 180)) / Math.PI) / 2 * 2 ** z)];
        for (const [name, lon, lat] of [['Todtnau', 7.95, 47.83], ['Murg', 8.36, 48.68], ['Rhine', 8.34, 49.07]]) {
            for (const [date, minute] of [['2026-06-21', 540], ['2026-06-21', 720], ['2026-06-21', 1140], ['2026-12-21', 540], ['2026-12-21', 720], ['2026-12-21', 900]]) {
                const [x, y] = tile(lon, lat, 12);
                for (const phase of ['new', 'cached']) {
                    const before = structuredClone(client.metrics.stats), start = performance.now();
                    const values = await client.tile(12, x, y, date, minute, new AbortController().signal);
                    const counts = [0, 0, 0, 0, 0]; values.forEach(v => counts[v]++);
                    const after = client.metrics.stats;
                    rows.push({ name, date, minute, phase, ms: +(performance.now() - start).toFixed(2), cpuMs: +((after.cpuMs - (before?.cpuMs ?? 0))).toFixed(2), requests: after.requests - (before?.requests ?? 0), bytes: after.bytes - (before?.bytes ?? 0), decodedMB: +(after.decodedBytes / 1e6).toFixed(2), counts });
                    if (phase === 'new') for (const i of [2056, 6177, 10312, 14440]) {
                        const px = x + (i % 128 + .5) / 128, py = y + (Math.floor(i / 128) + .5) / 128;
                        checks.push({ coordinate: [px / 2 ** 12 * 360 - 180, Math.atan(Math.sinh(Math.PI * (1 - 2 * py / 2 ** 12))) * 180 / Math.PI], date, minute, overview: values[i] });
                    }
                }
            }
        }
        const start = performance.now();
        const day = await client.day([7.95, 47.83], '2026-06-21', new AbortController().signal);
        rows.push({ dayMs: +(performance.now() - start).toFixed(2), dayUnknown: [...day.states].filter(v => v === 3).length });
        const [x, y] = tile(7.95, 47.83, 12);
        const abort = new AbortController();
        const stale = client.tile(12, x, y, '2026-12-21', 545, abort.signal).catch(() => 'cancelled');
        const cancelStart = performance.now();
        abort.abort();
        await stale;
        await client.tile(12, x, y, '2026-06-21', 720, new AbortController().signal);
        rows.push({ cancelThenCachedMs: +(performance.now() - cancelStart).toFixed(2) });
        const viewport = async (name, minute, offset = 0, zoom = 11) => {
            const [vx, vy] = tile(8.36, 48.68, zoom);
            const before = structuredClone(client.metrics.stats), start = performance.now();
            const tiles = await Promise.all(Array.from({ length: 12 }, (_, i) => client.tile(zoom, vx + i % 4 - 1 + offset, vy + Math.floor(i / 4) - 1, '2026-12-21', minute, new AbortController().signal)));
            const counts = [0, 0, 0, 0, 0]; tiles.forEach(values => values.forEach(v => counts[v]++));
            const after = client.metrics.stats;
            rows.push({ viewport: name, zoom, tiles: 12, ms: +(performance.now() - start).toFixed(2), cpuMs: +(after.cpuMs - before.cpuMs).toFixed(2), requests: after.requests - before.requests, bytes: after.bytes - before.bytes, decodedMB: +(after.decodedBytes / 1e6).toFixed(2), counts });
        };
        await viewport('first winter view', 540);
        await viewport('same view cached', 540);
        await viewport('pan one tile east', 540, 1);
        await viewport('change time', 720, 1);
        for (const zoom of [9, 8, 7, 6]) {
            await viewport('wide first view', 540, 0, zoom);
            await viewport('wide change time', 720, 0, zoom);
            await viewport('wide pan one tile', 720, 1, zoom);
        }
        await viewport('wide sunset', 985, 0, 6);
        const pending = new AbortController();
        const [vx, vy] = tile(8.36, 48.68, 11);
        const obsolete = Array.from({ length: 12 }, (_, i) => client.tile(11, vx + i % 4 - 1, vy + Math.floor(i / 4) - 1, '2026-12-21', 545, pending.signal).catch(() => 'cancelled'));
        const interrupted = performance.now();
        await new Promise(resolve => setTimeout(resolve, 10));
        pending.abort();
        await Promise.all(obsolete);
        await client.tile(11, vx, vy, '2026-12-21', 720, new AbortController().signal);
        rows.push({ cancelViewportThenCachedMs: +(performance.now() - interrupted).toFixed(2) });
        let different = 0, unknown = 0;
        for (const check of checks) {
            const [native] = await client.sample(check.coordinate, check.date, check.minute, new AbortController().signal);
            if (native === 3 || check.overview === 3) unknown++;
            else if (native !== check.overview) different++;
        }
        rows.push({ nativeComparison: checks.length, different, unknown });
        const before = JSON.stringify(client.metrics.stats);
        await new Promise(resolve => setTimeout(resolve, 3000));
        rows.push({ idleChanged: JSON.stringify(client.metrics.stats) !== before });
        client.dispose();
        return rows;
    }, new URL(`/@fs${assets}/${bundle}`, base).href);
    console.log(JSON.stringify(result, null, 2));
} finally { await browser.close(); }
