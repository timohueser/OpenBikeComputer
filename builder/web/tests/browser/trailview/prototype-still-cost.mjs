// PROTOTYPE for #3361, never merged: the cost of one TrailView still on this machine's GPU.
//
//   node builder/web/tests/browser/trailview/prototype-still-cost.mjs --jobs FILE --dist DIR [--out DIR] [--warm]
//
// --jobs is a JSON list of {name, view, width, height, route?}, route a file of [[lon, lat], ...].
// --dist is a TrailView page built against a catalogue (VITE_PLANNER_CATALOG). Each job gets a new
// browser (an empty cache, a new profile); --warm loads it a second time in the same browser.
// --mirror DIR answers every request off this machine from DIR, and fetches and keeps one that DIR
// lacks: a first run fills it over the network, later runs read the release locally. Without
// --mirror the page loads over the real network.
// Prints one JSON line per load and writes the PNG, the WebP 0.85 and the AVIF of each image.
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { parseArgs } from 'node:util';
import { errorsOf, launch, serve } from './harness.js';

const { values: a } = parseArgs({ options: { jobs: { type: 'string' }, dist: { type: 'string' }, out: { type: 'string', default: '.artifacts/still-cost' }, warm: { type: 'boolean' }, avif: { type: 'boolean' }, mirror: { type: 'string' } } });
const jobs = JSON.parse(readFileSync(a.jobs, 'utf8'));
mkdirSync(a.out, { recursive: true });
const { origin, close } = await serve(a.dist.endsWith('/') ? a.dist : `${a.dist}/`);
const TIMEOUT_MS = 600_000;
if (a.mirror) mkdirSync(a.mirror, { recursive: true });
const mirror = { hit: 0, fetched: 0 };

async function answerFromMirror(route) {
    const request = route.request(), key = join(a.mirror, createHash('sha256').update(`${request.method()} ${request.url()} ${request.headers().range ?? ''}`).digest('hex'));
    if (!existsSync(key)) {
        const response = await route.fetch();
        // body() is decoded, so only the type and the range go with it.
        const got = response.headers(), headers = { 'content-type': got['content-type'] ?? 'application/octet-stream' };
        if (got['content-range']) headers['content-range'] = got['content-range'];
        writeFileSync(`${key}.json`, JSON.stringify({ status: response.status(), headers }));
        writeFileSync(key, await response.body());
        mirror.fetched++;
    } else mirror.hit++;
    const { status, headers } = JSON.parse(readFileSync(`${key}.json`, 'utf8'));
    return route.fulfill({ status, body: readFileSync(key), headers: { ...headers, 'access-control-allow-origin': '*' } });
}

async function load(context, job, run) {
    const page = await context.newPage();
    const errors = errorsOf(page);
    if (a.mirror) await page.route(url => url.hostname !== '127.0.0.1', answerFromMirror);
    // The bytes that cross the network: a load from the browser cache adds about nothing.
    const cdp = await context.newCDPSession(page), wire = { bytes: 0, cached: 0 };
    await cdp.send('Network.enable');
    cdp.on('Network.requestServedFromCache', () => wire.cached++);
    const external = new Set();
    cdp.on('Network.requestWillBeSent', e => { if (!e.request.url.startsWith(origin)) external.add(e.requestId); });
    cdp.on('Network.loadingFinished', e => { if (external.has(e.requestId)) wire.bytes += e.encodedDataLength; });
    const route = job.route && JSON.parse(readFileSync(job.route, 'utf8'));
    const t0 = performance.now();
    await page.goto(`${origin}/trailview.html${job.view}`);
    await page.waitForFunction(() => window.trailView, null, { timeout: TIMEOUT_MS });
    const started = performance.now();
    await page.addStyleTag({ content: '#hud, #credits { display: none; }' });
    await page.evaluate(route => {
        window.trailView.showMarker(null);
        if (route) window.trailView.showRoute(route, route.slice(1).map(() => '#e8452c'));
    }, route);
    await page.evaluate(ms => Promise.race([window.trailView.waitForIdle(), new Promise((_, no) => setTimeout(() => no(new Error('not idle')), ms))]), TIMEOUT_MS);
    const idle = performance.now();
    const stats = await page.evaluate(() => { const s = window.trailView.stats(); return { adapter: s.adapter, requests: s.requests, bytes: s.bytes, failed: s.failed, drawn: s.drawn, scale: s.scale, pools: Object.fromEntries(s.pools) }; });
    const png = join(a.out, `${job.name}-${run}.png`);
    await page.screenshot({ path: png });
    const webp = png.replace(/\.png$/, '.webp'), avif = png.replace(/\.png$/, '.avif');
    execFileSync('cwebp', ['-quiet', '-q', '85', png, '-o', webp]);
    const sizes = { webp: statSync(webp).size };
    if (a.avif) { execFileSync('magick', [png, '-quality', '60', avif]); sizes.avif = statSync(avif).size; }
    const pools = Object.values(stats.pools).reduce((s, b) => s + b, 0);
    console.log(JSON.stringify({ name: job.name, run, start_s: +((started - t0) / 1000).toFixed(1), idle_s: +((idle - t0) / 1000).toFixed(1), ...stats, pools_mb: +(pools / 2 ** 20).toFixed(0), wire_mb: +(wire.bytes / 1e6).toFixed(1), from_cache: wire.cached, ...sizes, errors }));
    await page.close();
}

try {
    for (const job of jobs) {
        const browser = await launch(true);
        const context = await browser.newContext({ viewport: { width: job.width, height: job.height }, deviceScaleFactor: 1 });
        await load(context, job, 'cold').catch(e => console.log(JSON.stringify({ name: job.name, run: 'cold', error: e.message })));
        if (a.warm) await load(context, job, 'warm').catch(e => console.log(JSON.stringify({ name: job.name, run: 'warm', error: e.message })));
        await browser.close();
    }
} finally {
    close();
}
