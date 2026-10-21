// Flies scripted paths through the built TrailView page, one frame per step, and prints per flight
// the GPU time per pass, the CPU time, the terrain selection, nodes, triangles and pages, and the
// requests.
//
//   npm run pretest:trailview --prefix builder/web/tests/browser     # build against the fixture
//   node builder/web/tests/browser/trailview/flight.mjs --gpu [--flights FILE] [--tiles FILE] [--features DIR] [--lab LAB] [--cracks N] [--audit N]
//
// --flights is a JSON list of {name, from, to, frames, viewport?}: a view as a query string, the
// lat, lon, heading, pitch and height where the flight ends, the frames between (default:
// flights.json), and the CSS pixels of the page (default 1280 × 800). --tiles serves a
// `planner-dem` MBTiles instead of the fixture, --features a folder of feature tiles `z/x/y.bin`.
// The stamp line is the GPU time of the frames with a stamp pass and the pages that they stamp.
// --lab adds `lab=LAB` to each view. --cracks N
// draws with the red clear colour and counts the red pixels of every Nth frame as it is, pages in
// flight too: the flight must look down on known terrain. --audit N waits until every Nth frame is
// idle and adds up its invariant audit (`trailView.audit()`). --gpu draws 2560 × 1600 pixels on the machine's GPU in a browser window,
// with exact timestamps. Without it SwiftShader draws on the CPU, and its times say nothing about a GPU.

import { readFileSync } from 'node:fs';
import { parseArgs } from 'node:util';
import { chromium } from 'playwright';
import { EXECUTABLE, redPixels, routeFixture, serve, SWIFTSHADER } from './harness.js';

const { values } = parseArgs({
    options: {
        flights: { type: 'string', default: new URL('./flights.json', import.meta.url).pathname },
        tiles: { type: 'string' },
        features: { type: 'string' },
        lab: { type: 'string' },
        cracks: { type: 'string' },
        audit: { type: 'string' },
        gpu: { type: 'boolean' },
    },
});
const flights = JSON.parse(readFileSync(values.flights, 'utf8'));
const { origin, close } = await serve();
const browser = await chromium.launch({
    executablePath: EXECUTABLE, headless: !values.gpu,
    args: values.gpu ? ['--enable-webgpu-developer-features'] : SWIFTSHADER,
});
/**
 * Keeps the GPU at its high clock: macOS lowers the clock while a frame leaves the GPU time to spare, so
 * a pass then measures much longer than its work. Before each submit of the
 * engine, about 5 ms of compute writes the node buffer without a change, so the terrain pass waits for
 * it and never overlaps it. It reaches into the engine, so it lives here and not in the engine.
 */
function heat() {
    const device = window.trailView.gpuState.gpu.device, nodes = window.trailView.terrain.resources.nodes;
    const module = device.createShaderModule({ code: `
@group(0) @binding(0) var<storage, read_write> nodes: array<u32>;
@compute @workgroup_size(64) fn main(@builtin(global_invocation_id) id: vec3u) {
    var x = f32(id.x) * 1e-6;
    for (var i = 0u; i < 20000u; i++) { x = fma(x, 0.999999, 1e-7); }
    nodes[id.x % 1024u] = nodes[id.x % 1024u] | select(0u, 1u, x < -1.0);
}` });
    const pipeline = device.createComputePipeline({ layout: 'auto', compute: { module, entryPoint: 'main' } });
    const group = device.createBindGroup({ layout: pipeline.getBindGroupLayout(0), entries: [{ binding: 0, resource: { buffer: nodes } }] });
    const submit = device.queue.submit.bind(device.queue);
    device.queue.submit = buffers => {
        const encoder = device.createCommandEncoder(), pass = encoder.beginComputePass();
        pass.setPipeline(pipeline);
        pass.setBindGroup(0, group);
        pass.dispatchWorkgroups(1024);
        pass.end();
        submit([encoder.finish(), ...buffers]);
    };
}

const summary = (list) => {
    const sorted = [...list].sort((a, b) => a - b), at = (q) => sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))];
    return sorted.length ? { median: +at(0.5).toFixed(2), p95: +at(0.95).toFixed(2), max: +sorted.at(-1).toFixed(2) } : null;
};
try {
    for (const flight of flights) {
        const [width, height] = flight.viewport ?? [1280, 800];
        const page = await browser.newPage({ viewport: { width, height }, deviceScaleFactor: 2 });
        await routeFixture(page, values.tiles, values.features);
        const from = values.lab ? `${flight.from}&lab=${values.lab}` : flight.from;
        await page.goto(`${origin}/trailview.html${values.cracks ? `${from}&debug=cracks` : from}`);
        await page.waitForFunction(() => window.trailView, null, { timeout: 60_000 });
        await page.evaluate(() => window.trailView.waitForIdle());
        if (values.cracks) await page.addStyleTag({ content: '#hud { display: none; }' });
        if (values.gpu) await page.evaluate(heat);
        const start = await page.evaluate(() => ({ ...window.trailView.view }));
        const frames = [], red = [], audit = { checks: {}, levels: { lakes: 0, mixed: 0 }, rivers: { checked: 0, rising: 0 }, apron: { checked: 0, differ: 0 }, found: [] };
        const stops = [values.cracks, values.audit].filter(Boolean).map(Number);
        // The frames run in the page, one per display frame, as a rider's flight does: a round trip per
        // frame leaves the GPU idle, and macOS then lowers its clock. Only a crack count or an audit leaves the page.
        for (let i = 0; i <= flight.frames;) {
            const until = Math.min(flight.frames, ...stops.map(n => Math.ceil(i / n) * n));
            frames.push(...await page.evaluate(async ({ start, to, frames, from, until }) => {
                const tv = window.trailView, end = { ...start, ...to }, out = [];
                for (let i = from; i <= until; i++) {
                    const t = i / frames, mix = (a, b) => a + (b - a) * t;
                    // A flight that climbs or sinks changes its height by the same share each frame.
                    const height = start.mode === 'fly' ? start.height * (end.height / start.height) ** t : mix(start.height, end.height);
                    tv.view = { ...start, lat: mix(start.lat, end.lat), lon: mix(start.lon, end.lon), heading: mix(start.heading, end.heading), pitch: mix(start.pitch, end.pitch), height };
                    await tv.nextFrame();
                    const { gpu, cpu, drawn, requests, bytes, terrain } = tv.stats();
                    out.push({ gpu, cpu, drawn, requests, bytes, terrain });
                }
                return out;
            }, { ...flight, start, from: i, until }));
            if (values.cracks && until % Number(values.cracks) === 0) red.push(await redPixels(page));
            if (values.audit && until % Number(values.audit) === 0) {
                const result = await page.evaluate(async () => {
                    await window.trailView.waitForIdle();
                    return window.trailView.audit();
                });
                for (const [name, check] of Object.entries(result.checks)) {
                    const sum = audit.checks[name] ??= { checked: 0, violations: 0, conflicts: 0, largest: 0 };
                    for (const key of ['checked', 'violations', 'conflicts']) sum[key] += check[key];
                    sum.largest = Math.max(sum.largest, check.largest);
                }
                for (const part of ['levels', 'rivers', 'apron']) for (const key of Object.keys(audit[part])) audit[part][key] += result[part][key];
                audit.found.push(...result.found.slice(0, 8 - audit.found.length));
            }
            i = until + 1;
        }
        const passes = [...new Set(frames.flatMap(frame => Object.keys(frame.gpu)))];
        const last = frames.at(-1), stats = await page.evaluate(() => ({ ...window.trailView.stats(), pools: [...window.trailView.stats().pools] }));
        const of = key => frames.map(frame => frame.terrain[key]);
        console.log(JSON.stringify({
            flight: flight.name, adapter: stats.adapter, frames: frames.length,
            gpu: Object.fromEntries(passes.map(pass => [pass, summary(frames.map(frame => frame.gpu[pass]).filter(ms => ms !== undefined))])),
            stamp: { ms: summary(frames.map(frame => frame.gpu.stamp).filter(ms => ms !== undefined)), pages: summary(of('stamps').filter(pages => pages > 0)) },
            cpu: summary(frames.map(frame => frame.cpu)), selection: summary(of('selection')),
            nodes: summary(of('nodes')), triangles: summary(of('triangles')), quality: summary(of('quality')),
            pages: { stored: Math.max(...of('resident').map(r => r.stored)), synth: Math.max(...of('resident').map(r => r.synth)), capacity: last.terrain.capacity },
            poolMegabytes: Object.fromEntries(stats.pools.map(([pool, bytes]) => [pool, +(bytes / 2 ** 20).toFixed(1)])),
            requests: last.requests, megabytes: +(last.bytes / 2 ** 20).toFixed(2),
            ...(values.cracks ? { redPixels: red } : {}),
            ...(values.audit ? { audit } : {}),
        }));
        await page.close();
    }
} finally {
    await browser.close();
    close();
}
