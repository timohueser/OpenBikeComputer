// PROTOTYPE route-still (never merges). Can a page draw a route over a TrailView still from the still,
// a depth image and the camera values alone?
//
//   npm run pretest:trailview --prefix builder/web/tests/browser
//   node builder/web/tests/browser/trailview/prototype-route-still.mjs [--width PX] [--height PX] [--out DIR] [--offset] [--replay] [--heights FILE]
//
// Renders the still of prototype-route.json without a route, reads the hook of the engine, renders the
// engine's own ribbon from the same view, then draws the line in a page WITHOUT the engine and prints the
// measurements. --offset starts the engine elsewhere, so the frame's origin is not the eye. --replay
// skips the renders and reads the files of an earlier run in --out. --heights is a JSON list of other
// heights of the 2,000 points (points.json of --out), to compare with the drawn ones.

import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { parseArgs } from 'node:util';
import { errorsOf, launch, routeFixture, serve } from './harness.js';

const { values } = parseArgs({
    options: {
        out: { type: 'string', default: new URL('../../../../../.artifacts/prototype-route-still', import.meta.url).pathname },
        width: { type: 'string', default: '960' },
        height: { type: 'string', default: '540' },
        offset: { type: 'boolean' },
        replay: { type: 'boolean' },
        heights: { type: 'string' },
    },
});
const IDLE_MS = 30 * 60_000, POINTS = 2000, COLOUR = '#ff5a1f';
const width = Number(values.width), height = Number(values.height), out = values.out;
const route = JSON.parse(readFileSync(new URL('./prototype-route.json', import.meta.url), 'utf8'));
mkdirSync(out, { recursive: true });

/** `n` points evenly along the route, lon and lat. */
function dense(n) {
    const along = [0];
    for (let i = 1; i < route.length; i++) along.push(along[i - 1] + Math.hypot((route[i][0] - route[i - 1][0]) * Math.cos(route[i][1] * Math.PI / 180), route[i][1] - route[i - 1][1]));
    const points = [];
    for (let k = 0, i = 1; k < n; k++) {
        const s = along.at(-1) * k / (n - 1);
        while (i < route.length - 1 && along[i] < s) i++;
        const t = (s - along[i - 1]) / (along[i] - along[i - 1]);
        points.push([route[i - 1][0] + (route[i][0] - route[i - 1][0]) * t, route[i - 1][1] + (route[i][1] - route[i - 1][1]) * t]);
    }
    return points;
}

const { origin, close } = await serve();
const browser = await launch(false);
const idle = page => page.evaluate(ms => Promise.race([
    window.trailView.waitForIdle(),
    new Promise((_, reject) => setTimeout(() => reject(new Error(`not idle after ${ms} ms`)), ms)),
]), IDLE_MS);
const query = view => `?lat=${view.lat.toFixed(6)}&lon=${view.lon.toFixed(6)}&heading=${view.heading.toFixed(1)}&pitch=${view.pitch.toFixed(1)}&alt=${view.height.toFixed(1)}&date=2026-08-15&time=15:00&weather=sunny`;

try {
    // The page without the engine: it frames the route, and later draws the line.
    const plain = await browser.newPage({ viewport: { width, height }, deviceScaleFactor: 1 });
    const plainErrors = errorsOf(plain);
    await plain.goto(`${origin}/trailview.html?prototype=still`);
    await plain.waitForFunction(() => window.prototypeRouteStill);

    if (!values.replay) {
        const framed = await plain.evaluate(([route, aspect]) => window.prototypeRouteStill.frameRoute(route, aspect), [route, width / height]);
        console.log('view', query(framed));
        const page = await browser.newPage({ viewport: { width, height }, deviceScaleFactor: 1 });
        const errors = errorsOf(page);
        await routeFixture(page);
        const started = Date.now();
        await page.goto(`${origin}/trailview.html${query(values.offset ? { ...framed, lon: framed.lon + 0.03, lat: framed.lat + 0.015 } : framed)}`);
        await page.addStyleTag({ content: '#hud, #credits { display: none; }' });
        await page.waitForFunction(() => window.trailView, null, { timeout: IDLE_MS });
        if (values.offset) await page.evaluate(view => { window.trailView.view = { ...window.trailView.view, ...view }; }, framed);
        await idle(page);
        const seconds = (Date.now() - started) / 1000;
        await page.screenshot({ path: join(out, 'still.png') });
        const stats = await page.evaluate(() => {
            const { adapter, requests, bytes, failed, drawn, scale } = window.trailView.stats();
            return { adapter, requests, bytes, failed, drawn, scale };
        });
        const hookStarted = Date.now();
        const { depth, ...camera } = await page.evaluate(async () => {
            const still = await window.trailView.prototypeStill(), bytes = new Uint8Array(still.depth.buffer);
            let text = '';
            for (let i = 0; i < bytes.length; i += 0x8000) text += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
            return { ...still, depth: btoa(text) };
        });
        const hookSeconds = (Date.now() - hookStarted) / 1000;
        writeFileSync(join(out, 'depth.f32'), Buffer.from(depth, 'base64'));
        const points = dense(POINTS), heightsStarted = Date.now();
        const heights = await page.evaluate(points => window.trailView.prototypeHeights(points), points);
        const heightsSeconds = (Date.now() - heightsStarted) / 1000;
        writeFileSync(join(out, 'points.json'), JSON.stringify(points.map((point, i) => [...point, heights[i]])));
        // The engine's own ribbon from the same view, to compare.
        const ribbonStarted = Date.now();
        await page.evaluate(([route, colour]) => window.trailView.showRoute(route.map(([lon, lat]) => [lon, lat]), route.slice(1).map(() => colour)), [route, COLOUR]);
        await idle(page);
        await page.screenshot({ path: join(out, 'engine-ribbon.png') });
        const ribbonSeconds = (Date.now() - ribbonStarted) / 1000;
        writeFileSync(join(out, 'camera.json'), JSON.stringify({ ...camera, stats, seconds, hookSeconds, heightsSeconds, ribbonSeconds, offset: !!values.offset, errors }, null, 1));
        await page.close();
    }

    const { stats, seconds, hookSeconds, heightsSeconds, ribbonSeconds, offset, errors, ...camera } = JSON.parse(readFileSync(join(out, 'camera.json'), 'utf8'));
    const base64 = name => readFileSync(join(out, name)).toString('base64');
    const result = await plain.evaluate(async ({ camera, depth, points, other, still, ribbon }) => {
        const P = window.prototypeRouteStill;
        const bytes = async text => new Uint8Array(await (await fetch(`data:application/octet-stream;base64,${text}`)).arrayBuffer());
        const bitmap = async text => createImageBitmap(new Blob([await bytes(text)], { type: 'image/png' }));
        const encoded = async (canvas, type, quality) => {
            const blob = await canvas.convertToBlob({ type, quality });
            const data = await new Promise(resolve => {
                const reader = new FileReader();
                reader.onload = () => resolve(reader.result.slice(reader.result.indexOf(',') + 1));
                reader.readAsDataURL(blob);
            });
            return { bytes: blob.size, data };
        };
        const median = list => [...list].sort((a, b) => a - b)[list.length >> 1];
        const timed = async (work, runs = 21) => {
            const times = [];
            for (let k = 0; k < runs; k++) {
                const start = performance.now();
                await work();
                times.push(performance.now() - start);
            }
            return median(times);
        };
        const [W, H] = camera.output, [w, h] = camera.size, n = points.length;
        const flat = Float64Array.from(points.flat());
        const known = points.filter(p => Number.isFinite(p[2])).length;
        const stillImage = await bitmap(still), ribbonImage = await bitmap(ribbon);
        const full = P.depthImage(new Float32Array((await bytes(depth)).buffer), w, h, camera.near);
        const base = P.project(camera, flat, full);
        const visible = base.visible.reduce((a, b) => a + b, 0);
        // The points whose test differs from the full float depth, and the pixels of the line that they stand for.
        const step = i => (i > 0 && i < n ? Math.hypot(base.xy[2 * i] - base.xy[2 * i - 2], base.xy[2 * i + 1] - base.xy[2 * i - 1]) || 0 : 0);
        const flips = got => {
            let count = 0, pixels = 0;
            got.visible.forEach((v, i) => {
                if (v === base.visible[i]) return;
                count++;
                pixels += (step(i) + step(i + 1)) / 2;
            });
            return `${count} points, ${pixels.toFixed(0)} px of line`;
        };
        let linePixels = 0;
        for (let i = 1; i < n; i++) linePixels += step(i);
        const out = { output: [W, H], depthSize: [w, h], points: n, knownHeights: known, visible, hidden: n - visible, linePixels: Math.round(linePixels) };
        let finite = 0, near = Infinity, far = 0;
        for (const d of full.distance) if (Number.isFinite(d)) {
            finite++;
            near = Math.min(near, d);
            far = Math.max(far, d);
        }
        out.depthRange = { nearM: near, farM: far, skyShare: 1 - finite / full.distance.length };

        // The cost of the projection and the test of 2,000 points.
        out.projectMs = await timed(() => P.project(camera, flat, full), 101);

        // The depth image: raw, and 16-bit codes at full, half and quarter size, deflated.
        out.depth = { rawFloat32Bytes: 4 * w * h, rawFloat32DeflateBytes: (await P.deflate(await bytes(depth))).length, variants: [] };
        const images = {};
        for (const factor of [1, 2, 4]) for (const mode of factor === 1 ? ['first'] : ['far', 'near', 'first']) {
            const q = P.quantise(factor === 1 ? full : P.downsample(full, factor, mode));
            const plainBytes = await P.deflate(q.codes), packed = await P.deflate(P.delta(q.codes, q.width));
            const decodeMs = await timed(async () => P.dequantise({ ...q, codes: P.delta(new Uint16Array((await P.deflate(packed, true)).buffer), q.width, true) }), 11);
            const back = P.dequantise({ ...q, codes: P.delta(new Uint16Array((await P.deflate(packed, true)).buffer), q.width, true) });
            images[`${factor}-${mode}`] = back;
            out.depth.variants.push({
                factor, mode, size: [q.width, q.height], uint16Bytes: 2 * q.codes.length, deflateBytes: plainBytes.length, deltaDeflateBytes: packed.length, decodeMs,
                stepM: [q.min * (Math.exp(Math.log(q.max / q.min) / 65534) - 1), q.max * (1 - Math.exp(-Math.log(q.max / q.min) / 65534))],
                flips: flips(P.project(camera, flat, back)), flipsBilinear: flips(P.project(camera, flat, back, { bilinear: true })),
                flipsPull8: flips(P.project(camera, flat, back, { pullPixels: 8 })),
            });
        }

        // Heights that are off: a constant error, and another source if the script got one.
        const shifted = dz => {
            const moved = Float64Array.from(flat, (v, i) => (i % 3 === 2 ? v + dz : v)), got = P.project(camera, moved, full);
            let shift = 0;
            for (let i = 0; i < 2 * n; i += 2) shift = Math.max(shift, Math.hypot(got.xy[i] - base.xy[i], got.xy[i + 1] - base.xy[i + 1]) || 0);
            return { dz, flips: flips(got), maxShiftPixels: shift };
        };
        out.heightError = [-30, -10, -3, 3, 10, 30].map(shifted);
        if (other) {
            const moved = Float64Array.from(flat, (v, i) => (i % 3 === 2 ? other[(i - 2) / 3] : v)), got = P.project(camera, moved, full);
            let shift = 0, worst = 0, sum = 0;
            for (let i = 0; i < n; i++) {
                shift = Math.max(shift, Math.hypot(got.xy[2 * i] - base.xy[2 * i], got.xy[2 * i + 1] - base.xy[2 * i + 1]) || 0);
                worst = Math.max(worst, Math.abs(other[i] - points[i][2]) || 0);
                sum += Math.abs(other[i] - points[i][2]) || 0;
            }
            out.otherHeights = { flips: flips(got), maxShiftPixels: shift, maxDifferenceM: worst, meanDifferenceM: sum / n };
        }

        // The camera from the view alone: the frame about the eye, and the matrix from heading, pitch and aspect.
        const { view } = camera;
        const alone = { ...camera, origin: [view.lon, view.lat], eye: [0, 0, view.height], viewProj: Array.from(P.viewProjection(view.heading, view.pitch, W / H)), jitter: [0, 0] };
        const got = P.project(alone, flat, full);
        let shift = 0;
        for (let i = 0; i < 2 * n; i += 2) shift = Math.max(shift, Math.hypot(got.xy[i] - base.xy[i], got.xy[i + 1] - base.xy[i + 1]) || 0);
        out.fromViewAlone = { flips: flips(got), maxShiftPixels: shift, eyeInFrame: camera.eye, jitter: camera.jitter };

        // The still as WebP.
        const canvas = new OffscreenCanvas(W, H), context = canvas.getContext('2d', { willReadFrequently: true });
        context.drawImage(stillImage, 0, 0);
        out.webpBytes = (await encoded(canvas, 'image/webp', 0.85)).bytes;
        out.jpegBytes = (await encoded(canvas, 'image/jpeg', 0.85)).bytes;

        // The engine's ribbon: where its image differs from the still, the ribbon shows.
        const a = context.getImageData(0, 0, W, H).data;
        context.drawImage(ribbonImage, 0, 0);
        const b = context.getImageData(0, 0, W, H).data, reach = Math.max(2, Math.round(W / 480));
        const shows = i => {
            const x = Math.round(base.xy[2 * i]), y = Math.round(base.xy[2 * i + 1]);
            for (let dy = -reach; dy <= reach; dy++) for (let dx = -reach; dx <= reach; dx++) {
                const px = x + dx, py = y + dy, at = 4 * (py * W + px);
                if (px >= 0 && py >= 0 && px < W && py < H && Math.abs(a[at] - b[at]) + Math.abs(a[at + 1] - b[at + 1]) + Math.abs(a[at + 2] - b[at + 2]) > 60) return 1;
            }
            return 0;
        };
        const ribbonShows = Uint8Array.from({ length: n }, (_, i) => shows(i));
        const against = projected => {
            const counts = { both: 0, neither: 0, onlyPage: 0, onlyEngine: 0 };
            ribbonShows.forEach((v, i) => counts[v && projected.visible[i] ? 'both' : v ? 'onlyEngine' : projected.visible[i] ? 'onlyPage' : 'neither']++);
            return counts;
        };
        const quarter = P.project(camera, flat, images['4-far']);
        out.engineRibbon = { fullDepth: against(base), quarterFar: against(quarter), halfFar: against(P.project(camera, flat, images['2-far'])) };

        // The drawings: with the full depth and with the quarter depth of 16 bits.
        const drawn = async projected => {
            const start = performance.now();
            P.draw(canvas, stillImage, projected, Math.round(0.3 * n));
            const ms = performance.now() - start;
            return { ms, png: (await encoded(canvas, 'image/png')).data };
        };
        const [fullDrawing, quarterDrawing] = [await drawn(base), await drawn(quarter)];
        out.drawMs = fullDrawing.ms;
        return { out, full: fullDrawing.png, quarter: quarterDrawing.png, runs: Array.from(base.visible).join('').replace(/(.)\1*/g, s => `${s[0] === '1' ? 'v' : 'h'}${s.length} `) };
    }, {
        camera, depth: base64('depth.f32'), points: JSON.parse(readFileSync(join(out, 'points.json'), 'utf8')), other: values.heights ? JSON.parse(readFileSync(values.heights, 'utf8')) : null,
        still: base64('still.png'), ribbon: base64('engine-ribbon.png'),
    });
    writeFileSync(join(out, 'page-line-full-depth.png'), Buffer.from(result.full, 'base64'));
    writeFileSync(join(out, 'page-line-quarter-depth.png'), Buffer.from(result.quarter, 'base64'));
    console.log(JSON.stringify({ render: { seconds, hookSeconds, heightsSeconds, ribbonSeconds, offset, ...stats, errors }, ...result.out, plainErrors }, null, 1));
    console.log('visible (v) and hidden (h) runs along the route:', result.runs);
    console.log('files in', out);
} finally {
    await browser.close();
    close();
}
