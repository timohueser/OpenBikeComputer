// Flies scripted paths through the built TrailView page, one frame per step, and prints per flight
// the GPU time per pass, the CPU time, the tiles drawn and the requests.
//
//   npm run pretest:trailview --prefix builder/web/tests/browser     # build against the fixture
//   node builder/web/tests/browser/trailview/flight.mjs --gpu [--flights FILE]
//
// --flights is a JSON list of {name, from, to, frames}: a view as a query string, the lat, lon,
// heading, pitch and height where the flight ends, and the frames between (default: flights.json).
// --gpu draws 2560 × 1600 pixels on the machine's GPU in a browser window, with exact timestamps.
// Without it SwiftShader draws on the CPU, and its times say nothing about a GPU.

import { readFileSync } from 'node:fs';
import { parseArgs } from 'node:util';
import { chromium } from 'playwright';
import { EXECUTABLE, routeFixture, serve, SWIFTSHADER } from './harness.js';

const { values } = parseArgs({
    options: {
        flights: { type: 'string', default: new URL('./flights.json', import.meta.url).pathname },
        gpu: { type: 'boolean' },
    },
});
const flights = JSON.parse(readFileSync(values.flights, 'utf8'));
const { origin, close } = await serve();
const browser = await chromium.launch({
    executablePath: EXECUTABLE, headless: !values.gpu,
    args: values.gpu ? ['--enable-webgpu-developer-features'] : SWIFTSHADER,
});
const summary = (list) => {
    const sorted = [...list].sort((a, b) => a - b), at = (q) => sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))];
    return sorted.length ? { median: +at(0.5).toFixed(2), p95: +at(0.95).toFixed(2), max: +sorted.at(-1).toFixed(2) } : null;
};
try {
    for (const flight of flights) {
        const page = await browser.newPage({ viewport: { width: 1280, height: 800 }, deviceScaleFactor: 2 });
        await routeFixture(page);
        await page.goto(`${origin}/trailview.html${flight.from}`);
        await page.waitForFunction(() => window.trailView, null, { timeout: 60_000 });
        await page.evaluate(() => window.trailView.waitForIdle());
        const frames = await page.evaluate(async ({ to, frames }) => {
            const tv = window.trailView, start = { ...tv.view }, end = { ...start, ...to };
            const log = [];
            for (let i = 0; i <= frames; i++) {
                const t = i / frames, mix = (a, b) => a + (b - a) * t;
                // A flight that climbs or sinks changes its height by the same share each frame.
                const height = start.mode === 'fly' ? start.height * (end.height / start.height) ** t : mix(start.height, end.height);
                tv.view = { ...start, lat: mix(start.lat, end.lat), lon: mix(start.lon, end.lon), heading: mix(start.heading, end.heading), pitch: mix(start.pitch, end.pitch), height };
                await tv.nextFrame();
                const { gpu, cpu, drawn, requests, bytes } = tv.stats();
                log.push({ gpu, cpu, drawn, requests, bytes });
            }
            return log;
        }, flight);
        const passes = [...new Set(frames.flatMap(frame => Object.keys(frame.gpu)))];
        const last = frames.at(-1);
        console.log(JSON.stringify({
            flight: flight.name, adapter: await page.evaluate(() => window.trailView.stats().adapter), frames: frames.length,
            gpu: Object.fromEntries(passes.map(pass => [pass, summary(frames.map(frame => frame.gpu[pass]).filter(ms => ms !== undefined))])),
            cpu: summary(frames.map(frame => frame.cpu)), drawn: summary(frames.map(frame => frame.drawn)),
            requests: last.requests, megabytes: +(last.bytes / 2 ** 20).toFixed(2),
        }));
        await page.close();
    }
} finally {
    await browser.close();
    close();
}
