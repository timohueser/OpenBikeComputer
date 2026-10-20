// Flies scripted paths through the built TrailView page, one frame per step, and prints per flight
// the GPU time per pass, the CPU time, the terrain selection, nodes, triangles and pages, and the
// requests.
//
//   npm run pretest:trailview --prefix builder/web/tests/browser     # build against the fixture
//   node builder/web/tests/browser/trailview/flight.mjs --gpu [--flights FILE] [--tiles FILE] [--lab LAB] [--cracks N]
//
// --flights is a JSON list of {name, from, to, frames, viewport?}: a view as a query string, the
// lat, lon, heading, pitch and height where the flight ends, the frames between (default:
// flights.json), and the CSS pixels of the page (default 1280 × 800). --tiles serves a
// `planner-dem` MBTiles instead of the fixture. --lab adds `lab=LAB` to each view. --cracks N
// draws every Nth frame with the red clear colour and counts its red pixels: the flight must look
// down on known terrain. --gpu draws 2560 × 1600 pixels on the machine's GPU in a browser window,
// with exact timestamps. Without it SwiftShader draws on the CPU, and its times say nothing about a GPU.

import { readFileSync } from 'node:fs';
import { parseArgs } from 'node:util';
import { chromium } from 'playwright';
import { EXECUTABLE, redPixels, routeFixture, serve, SWIFTSHADER } from './harness.js';

const { values } = parseArgs({
    options: {
        flights: { type: 'string', default: new URL('./flights.json', import.meta.url).pathname },
        tiles: { type: 'string' },
        lab: { type: 'string' },
        cracks: { type: 'string' },
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
        const [width, height] = flight.viewport ?? [1280, 800];
        const page = await browser.newPage({ viewport: { width, height }, deviceScaleFactor: 2 });
        await routeFixture(page, values.tiles);
        const from = values.lab ? `${flight.from}&lab=${values.lab}` : flight.from;
        await page.goto(`${origin}/trailview.html${values.cracks ? `${from}&debug=cracks` : from}`);
        await page.waitForFunction(() => window.trailView, null, { timeout: 60_000 });
        await page.evaluate(() => window.trailView.waitForIdle());
        if (values.cracks) await page.addStyleTag({ content: '#hud { display: none; }' });
        const start = await page.evaluate(() => ({ ...window.trailView.view }));
        const frames = [], red = [];
        for (let i = 0; i <= flight.frames; i++) {
            const frame = await page.evaluate(async ({ start, to, frames, i }) => {
                const tv = window.trailView, end = { ...start, ...to }, t = i / frames, mix = (a, b) => a + (b - a) * t;
                // A flight that climbs or sinks changes its height by the same share each frame.
                const height = start.mode === 'fly' ? start.height * (end.height / start.height) ** t : mix(start.height, end.height);
                tv.view = { ...start, lat: mix(start.lat, end.lat), lon: mix(start.lon, end.lon), heading: mix(start.heading, end.heading), pitch: mix(start.pitch, end.pitch), height };
                await tv.nextFrame();
                const { gpu, cpu, drawn, requests, bytes, terrain } = tv.stats();
                return { gpu, cpu, drawn, requests, bytes, terrain };
            }, { ...flight, start, i });
            frames.push(frame);
            if (values.cracks && i % Number(values.cracks) === 0) {
                await page.evaluate(() => window.trailView.waitForIdle());
                red.push(await redPixels(page));
            }
        }
        const passes = [...new Set(frames.flatMap(frame => Object.keys(frame.gpu)))];
        const last = frames.at(-1), stats = await page.evaluate(() => window.trailView.stats());
        const of = key => frames.map(frame => frame.terrain[key]);
        console.log(JSON.stringify({
            flight: flight.name, adapter: stats.adapter, frames: frames.length,
            gpu: Object.fromEntries(passes.map(pass => [pass, summary(frames.map(frame => frame.gpu[pass]).filter(ms => ms !== undefined))])),
            cpu: summary(frames.map(frame => frame.cpu)), selection: summary(of('selection')),
            nodes: summary(of('nodes')), triangles: summary(of('triangles')), quality: summary(of('quality')),
            pages: { stored: Math.max(...of('resident').map(r => r.stored)), synth: Math.max(...of('resident').map(r => r.synth)), capacity: last.terrain.capacity },
            poolMegabytes: Object.fromEntries([...stats.pools].map(([pool, bytes]) => [pool, +(bytes / 2 ** 20).toFixed(1)])),
            requests: last.requests, megabytes: +(last.bytes / 2 ** 20).toFixed(2),
            ...(values.cracks ? { redPixels: red } : {}),
        }));
        await page.close();
    }
} finally {
    await browser.close();
    close();
}
