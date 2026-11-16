import { readdirSync } from 'node:fs';
import { expect, test } from '@playwright/test';
import { DIST, errorsOf, redPixels, routeFixture, screenPixels } from './harness.js';

// Down onto the middle of the fixture, so that only known terrain fills the frame and several levels meet.
const DOWN = '?lat=46.797500&lon=8.415000&heading=20.0&pitch=-60.0&alt=2500.0&date=2026-08-15&time=15:00&weather=sunny';
// Straight down onto the roads of Engelberg from 100 m, where the finest level shows them, then onto Trübsee from 2 km.
const VILLAGE = '?lat=46.818000&lon=8.405000&heading=0.0&pitch=-89.0&alt=1100.0&date=2026-08-15&time=15:00&weather=sunny';
const LAKE = { lat: 46.7825, lon: 8.391, heading: 0, pitch: -35, mode: 'fly', height: 2400 };
// Straight down onto Engelberg from about 300 m, where a route draws a few pixels wide.
const ROUTES = '?lat=46.818000&lon=8.405000&heading=0.0&pitch=-89.0&alt=1300.0&date=2026-08-15&time=15:00&weather=sunny';

async function open(page, view) {
    await routeFixture(page);
    await page.goto(`/trailview.html${view}`);
    await page.waitForFunction(() => window.trailView, null, { timeout: 60_000 });
    await page.evaluate(() => window.trailView.waitForIdle());
}

/** Draws one frame of a debug view, once the pages are idle. */
async function debug(page, view) {
    await page.evaluate(async view => {
        const tv = window.trailView;
        tv.view = { ...tv.view, debug: view };
        await tv.nextFrame();
    }, view);
}

test('the page draws the fixture with no WebGPU error, and the probe gives the height that an instance shader evaluates, to 1 mm', { tag: '@gpu' }, async ({ page }) => {
    const errors = errorsOf(page);
    await open(page, DOWN);
    const { drawn, failed, terrain } = await page.evaluate(() => window.trailView.stats());
    expect(drawn).toBeGreaterThan(0);
    expect(terrain.roads.pieces).toBeGreaterThan(0);
    expect(failed).toBe(0);
    const points = Array.from({ length: 16 }, (_, i) => [8.415 + 0.002 * Math.cos(i), 46.7975 + 0.0015 * Math.sin(2 * i)]);
    const { probe, instance } = await page.evaluate(points => window.trailView.check(points), points);
    expect(probe.every(Number.isFinite)).toBe(true);
    for (const [i, height] of probe.entries()) expect(Math.abs(height - instance[i])).toBeLessThan(1e-3);
    expect(errors).toEqual([]);
});

test('the terrain has no crack, and drawnHeight is the drawn mesh: no pixel shows red', { tag: '@gpu' }, async ({ page }) => {
    const errors = errorsOf(page);
    await open(page, DOWN);
    await page.addStyleTag({ content: '#hud, #credits { display: none; }' });
    const { drawn, terrain } = await page.evaluate(() => window.trailView.stats());
    expect(drawn).toBeGreaterThan(0);
    expect(terrain.resident.synth).toBeGreaterThan(0);
    // The red clear colour shows through a crack.
    await debug(page, 'cracks');
    expect(await redPixels(page)).toBe(0);
    // The surface view evaluates drawnHeight per pixel, red where it leaves the mesh by 5 cm.
    await debug(page, 'surface');
    expect(await redPixels(page)).toBe(0);
    expect(errors).toEqual([]);
});

/**
 * The audit of the frame: no violation of a stamp at any level that the frame draws, one level per
 * lake, no river that rises downstream, and neighbouring nodes stamp the same texels.
 */
async function audited(page) {
    const audit = await page.evaluate(() => window.trailView.audit());
    for (const [name, check] of Object.entries(audit.checks)) expect(check.violations, `${name}: ${JSON.stringify(audit.found)}`).toBe(0);
    expect([audit.levels.mixed, audit.rivers.rising]).toEqual([0, 0]);
    expect(audit.apron.checked).toBeGreaterThan(0);
    expect(audit.apron.differ).toBe(0);
    return audit;
}

test('the stamps hold along the roads of a village, where debug=mask draws them, and in a lake at its one level', { tag: '@gpu' }, async ({ page }) => {
    const errors = errorsOf(page);
    await open(page, VILLAGE);
    const village = await audited(page);
    expect(village.checks.crossFall.checked).toBeGreaterThan(100);
    expect(village.checks.roadHeight.checked).toBeGreaterThan(100);
    await debug(page, 'mask');
    expect((await page.evaluate(() => window.trailView.stats())).terrain.masks).toBeGreaterThan(0);

    await page.evaluate(async lake => {
        const tv = window.trailView;
        tv.view = { ...tv.view, ...lake, debug: 'off' };
        await tv.waitForIdle();
    }, LAKE);
    const lake = await audited(page);
    expect(lake.checks.inWater.checked).toBeGreaterThan(50);
    expect(lake.checks.onBank.checked).toBeGreaterThan(0);
    expect(lake.levels.lakes).toBeGreaterThan(0);
    expect(errors).toEqual([]);
});

test('two routes leave the gap between them, routeAt finds each, and the bold route draws one time', { tag: '@gpu' }, async ({ page }) => {
    const errors = errorsOf(page);
    await open(page, ROUTES);
    await page.addStyleTag({ content: '#hud, #credits { display: none; }' });
    // West and east of the eye, from 40 m to 200 m from it, each with its profile every 30 m from the probe. Route 2 is black: the
    // middle of a line adds no light of its own and lets a twentieth of the ground through, at any width of the line.
    const routes = await page.evaluate(async () => {
        const tv = window.trailView, { lat, lon } = tv.view, degree = 111_320 * Math.cos(lat * Math.PI / 180);
        const entries = [0, 30, 60, 90, 120, 150, 160], lines = [[-200, -40], [40, 200]];
        const { probe } = await tv.check(lines.flatMap(([from, to]) => entries.map(d => [lon + (from + d * (to - from) / 160) / degree, lat])));
        return lines.map(([from, to], k) => ({
            line: [[lon + from / degree, lat], [lon + to / degree, lat]], colours: [['#00ffff', '#000000'][k]],
            profile: { step: 30, heights: probe.slice(7 * k, 7 * k + 7).map(h => (Number.isFinite(h) ? h : null)) },
        }));
    });
    /** The frame with these routes, once it is idle. */
    const draw = async (list, bold = null) => {
        await page.evaluate(async ([list, bold]) => {
            window.trailView.showRoutes(list);
            window.trailView.boldRoute(bold);
            await window.trailView.waitForIdle();
        }, [list, bold]);
        return screenPixels(page);
    };
    const none = await screenPixels(page), two = await draw(routes);
    const { width, height } = none, at = (shot, x, y) => [0, 1, 2].map(j => shot.rgba[4 * (y * width + x) + j]);
    const change = (shot, x, y, base = none) => at(shot, x, y).reduce((sum, value, j) => sum + Math.abs(value - at(base, x, y)[j]), 0);
    // The pixels that each route changes, route 1 west of the middle. A pixel of a route is the median of its pixels.
    const changed = [[], []];
    for (let y = 0; y < height; y++) for (let x = 0; x < width; x++) if (change(two, x, y) > 60) changed[x < width / 2 ? 0 : 1].push([x, y]);
    const median = values => values.sort((a, b) => a - b)[values.length >> 1];
    const [west, east] = changed.map(pixels => [median(pixels.map(p => p[0])), median(pixels.map(p => p[1]))]);
    expect(changed.map(pixels => pixels.length > 100)).toEqual([true, true]);
    // The gap is the middle fifth of the 400 m from the start of route 1 to the end of route 2: no pixel there changes.
    const xs = changed.flat().map(p => p[0]), reach = (Math.max(...xs) - Math.min(...xs)) / 10, row = Math.round((west[1] + east[1]) / 2);
    const gap = [];
    for (let x = Math.ceil(width / 2 - reach + 8); x < width / 2 + reach - 8; x++) for (let y = row - 3; y <= row + 3; y++) gap.push(change(two, x, y));
    expect(gap.length).toBeGreaterThan(500);
    expect(Math.max(...gap)).toBeLessThanOrEqual(12);
    expect(await page.evaluate(points => points.map(([x, y]) => window.trailView.routeAt(x + 0.5, y + 0.5)), [west, east, [width / 2, row]])).toEqual([1, 2, null]);

    const alone = await draw([routes[1]]), bold = await draw(routes, 2);
    // The middle of route 2 in its column: the row that the route alone changes most.
    const rows = y0 => Array.from({ length: 41 }, (_, k) => y0 - 20 + k), [x] = east;
    const middle = rows(east[1]).reduce((best, y) => (change(alone, x, y) > change(alone, x, best) ? y : best));
    const drawn = shot => rows(east[1]).filter(y => change(shot, x, y) > 60).length;
    expect(drawn(bold)).toBeGreaterThan(drawn(alone) + 2);
    // Drawn two times, the middle would let a four-hundredth of the ground through.
    expect(at(alone, x, middle)[1]).toBeGreaterThan(8);
    at(bold, x, middle).forEach((value, j) => expect(Math.abs(value - at(alone, x, middle)[j])).toBeLessThanOrEqual(3));
    // The second draw leaves route 1 as it was.
    expect(Math.max(...rows(west[1]).map(y => change(bold, west[0], y, two)))).toBeLessThanOrEqual(12);
    expect(errors).toEqual([]);
});

test('a ground worker keeps the ground bytes under a low alpha, the colour under the snow mark, and the run of the snow', async ({ page }) => {
    // Lossless WebP (cwebp -lossless -exact): a 512 px ground tile of material 13, barren share 77 and lithology 5, region 0 in its
    // west half and 2 in its east half; a premultiplied decode loses the bytes under these alphas. A quarter: an NDVI tile that
    // marks every texel with the snow mark, then the colour (90, 140, 60). And a 512 px snow tile whose run is bands 5 to 15 in its
    // west half and 20 to 30 in its east half: red the first band, green the last.
    const GROUND = 'UklGRjwAAABXRUJQVlA4TC8AAAAv/8F/EA8wTdOgIG0DFrY7IxAfBJm0zfyb3l0Fiej/BCx/zn/8x3/8x3/8x3+MBwA=';
    const QUARTER = 'UklGRhwAAABXRUJQVlA4TA8AAAAv/8A/AAfQqoj+ByKi/wEAUklGRi4AAABXRUJQVlA4TCIAAAAv/8F/AAdQxmqVp/8BgUCyv/cMRfQ/4z//+c9//vOf//wf';
    const SNOW = 'UklGRjYAAABXRUJQVlA4TCkAAAAv/8F/AA/Qhxc8xPyHgUzaZv5N766CRPR/ApY/5z/+4z/+4z/+4z/GAwA=';
    const worker = readdirSync(`${DIST}assets`).find(file => file.startsWith('ground-worker-'));
    await page.goto('/third-party-licenses.txt');
    const { probe, west, east } = await page.evaluate(async ({ worker, ground, quarter, snow }) => {
        const body = base64 => Uint8Array.from(atob(base64), c => c.charCodeAt(0)).buffer;
        const tile = (key, base64, layer) => ({ key, z: 3, x: 5, y: 2, body: body(base64), layer });
        const job = { z: 3, x: 5, y: 2, ground: tile('ground', ground, 'ground'), quarters: [tile('q1', quarter, 'colour'), undefined], snow: tile('snow', snow, 'snow'), date: { weight: 0, band: 10 }, world: 9, regionCount: 3 };
        const builder = new Worker(`/assets/${worker}`, { type: 'module' });
        const ask = message => new Promise(resolve => { builder.onmessage = ({ data }) => resolve(data); builder.postMessage(message); });
        const probe = await ask({ id: 0 });
        const { pages: [nw, ne] } = await ask({ id: 1, job });
        return { probe, west: [...nw.cells.subarray(0, 4), ...nw.colour.subarray(0, 4), ...nw.snow.subarray(0, 3)], east: [...ne.cells.subarray(0, 4), ...ne.snow.subarray(0, 2)] };
    }, { worker, ground: GROUND, quarter: QUARTER, snow: SNOW });
    expect(probe).toEqual({ id: 0 });
    // The material, the lithology, the region and the barren share; then the colour and the NDVI code of a quarter that saw snow; then
    // snow on band 10, snow data known and the snow mark.
    expect(west).toEqual([13, 5, 0, 77, 90, 140, 60, 128, 255, 255, 255]);
    expect(east).toEqual([13, 5, 2, 77, 0, 255]);
});
