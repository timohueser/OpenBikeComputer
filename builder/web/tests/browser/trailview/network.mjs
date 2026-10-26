// Loads a view of the box bake cold over a shaped link (link.mjs) on this machine's GPU, headless,
// and prints when the first frame, the near field and the idle view came, and what crossed the link.
//
//   node builder/web/tests/browser/trailview/network.mjs [--view NAME|QUERY] [--mbit 25] [--rtt 40] [--runs 1]
//     [--web DIR] [--tiles FILE] [--ground DIR] [--colour V] [--features DIR] [--port 8951] [--out FILE]
//
// --view is a name of views.json or a query string. --web is the `builder/web` whose page loads
// (default: this checkout's); the script builds it against the link. The data are the bake of box.sh;
// --tiles serves another terrain MBTiles, such as one in the release's encoding. --out writes the
// runs, with the log of every request, as JSON.
//
// The near field is every tile of a layer's finest zoom within NEAR_M of the eye and not behind it.
import { writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { parseArgs } from 'node:util';
import { chromium } from 'playwright';
import { buildPage, startLink } from './link.mjs';

const NEAR_M = 2000;
const BOX = '7.60,46.46,8.95,47.25';
const bake = join(process.env.TRAILVIEW_DIR ?? join(homedir(), '.cache/obc/trailview-box'), BOX);
const { values: a } = parseArgs({
    options: {
        view: { type: 'string', default: 'walk-engelberg-to-titlis' }, mbit: { type: 'string', default: '25' }, rtt: { type: 'string', default: '40' },
        runs: { type: 'string', default: '1' }, web: { type: 'string', default: new URL('../../..', import.meta.url).pathname },
        tiles: { type: 'string', default: join(bake, 'terrain.mbtiles') }, ground: { type: 'string', default: join(bake, 'ground') },
        colour: { type: 'string', default: 'webp-12' }, features: { type: 'string', default: join(bake, 'features') },
        port: { type: 'string', default: '8951' }, out: { type: 'string' },
    },
});
const views = (await import('./views.json', { with: { type: 'json' } })).default;
const query = views.find(v => v.name === a.view)?.view ?? a.view;
const params = new URLSearchParams(query), eye = { lat: +params.get('lat'), lon: +params.get('lon'), heading: +(params.get('heading') ?? 0) };
const port = +a.port, dist = join(a.web, 'dist/trailview-link');
buildPage({ web: a.web, tiles: `https://127.0.0.1:${port + 1}`, bounds: BOX.split(',').map(Number), out: dist });

/** The layer of a request and whether its tile is in the near field: not wholly behind the eye (each corner more than 100° off the heading). */
function place({ origin, path }) {
    if (origin === 'page') return { layer: 'page' };
    const m = /^\/(?:trailview-)?([a-z]+)[^/]*\/(\d+)\/(\d+)\/(\d+)$/.exec(path);
    if (!m) return { layer: 'json' };
    const [z, x, y] = m.slice(2).map(Number), n = 2 ** z;
    const lon = i => i / n * 360 - 180, lat = j => Math.atan(Math.sinh(Math.PI * (1 - 2 * j / n))) * 180 / Math.PI;
    const mx = 111320 * Math.cos(eye.lat * Math.PI / 180), my = 111320;
    const dx = Math.max(lon(x) - eye.lon, 0, eye.lon - lon(x + 1)) * mx, dy = Math.max(lat(y + 1) - eye.lat, 0, eye.lat - lat(y)) * my;
    const off = (i, j) => Math.abs(((Math.atan2((lon(i) - eye.lon) * mx, (lat(j) - eye.lat) * my) * 180 / Math.PI - eye.heading) % 360 + 540) % 360 - 180);
    const behind = dx + dy > 0 && [[x, y], [x + 1, y], [x, y + 1], [x + 1, y + 1]].every(([i, j]) => off(i, j) > 100);
    return { layer: m[1], z, near: Math.hypot(dx, dy) <= NEAR_M && !behind };
}

async function run() {
    const link = await startLink({ dist, terrain: a.tiles, ground: a.ground, colour: a.colour, features: a.features, mbit: +a.mbit, rtt: +a.rtt, port });
    const browser = await chromium.launch({ channel: 'chromium', headless: true, args: ['--enable-webgpu-developer-features', link.flag] });
    try {
        const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
        const errors = [];
        page.on('pageerror', error => errors.push(error.message));
        await page.addInitScript(() => {
            const id = setInterval(() => {
                if (window.trailView?.stats().drawn > 0) {
                    window.firstFrame = performance.now();
                    clearInterval(id);
                }
            }, 10);
        });
        await page.goto(`${link.page}/trailview.html${query}`);
        await page.waitForFunction(() => window.trailView, null, { timeout: 120_000 });
        const marks = await page.evaluate(async () => {
            await window.trailView.waitForIdle();
            return { start: performance.timeOrigin, first: window.firstFrame, idle: performance.now() };
        });
        const log = link.log.map(e => ({ ...e, ...place(e), t0: e.t0 - marks.start, te: e.te - marks.start }));
        return { marks, log, errors };
    } finally {
        await browser.close();
        await link.close();
    }
}

const MB = bytes => +(bytes / 1e6).toFixed(2);
function summary({ marks, log, errors }) {
    const layers = {};
    for (const { layer, bytes } of log) {
        layers[layer] ??= { requests: 0, MB: 0 };
        layers[layer].requests++;
        layers[layer].MB = +(layers[layer].MB + bytes / 1e6).toFixed(2);
    }
    const starts = log.map(e => e.t0).sort((p, q) => p - q);
    let burst = 0;
    for (let i = 0, j = 0; i < starts.length; i++) {
        while (starts[i] - starts[j] > 10_000) j++;
        burst = Math.max(burst, i - j + 1);
    }
    const finest = {};
    for (const { layer, z } of log) finest[layer] = Math.max(finest[layer] ?? 0, z ?? 0);
    const near = log.filter(e => e.near && e.z === finest[e.layer] && e.status === 200);
    return {
        'first frame s': +(marks.first / 1000).toFixed(2), 'near field s': +(Math.max(...near.map(e => e.te)) / 1000).toFixed(2),
        'idle s': +(marks.idle / 1000).toFixed(2), MB: MB(log.reduce((sum, e) => sum + e.bytes, 0)), requests: log.length,
        'near MB': MB(near.reduce((sum, e) => sum + e.bytes, 0)), 'burst per 10 s': burst, layers, errors,
    };
}

const runs = [];
for (let i = 0; i < +a.runs; i++) {
    runs.push(await run());
    console.log(JSON.stringify({ view: a.view, mbit: +a.mbit, rtt: +a.rtt, ...summary(runs.at(-1)) }));
}
if (a.out) writeFileSync(a.out, JSON.stringify({ args: a, runs }));
