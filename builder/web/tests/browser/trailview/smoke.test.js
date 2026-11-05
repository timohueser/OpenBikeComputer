import { readdirSync } from 'node:fs';
import { expect, test } from '@playwright/test';
import { DIST, errorsOf, redPixels, routeFixture } from './harness.js';

// Down onto the middle of the fixture, so that only known terrain fills the frame and several levels meet.
const DOWN = '?lat=46.797500&lon=8.415000&heading=20.0&pitch=-60.0&alt=2500.0&date=2026-08-15&time=15:00&weather=sunny';
// Straight down onto the roads of Engelberg from 100 m, where the finest level shows them, then onto Trübsee from 2 km.
const VILLAGE = '?lat=46.818000&lon=8.405000&heading=0.0&pitch=-89.0&alt=1100.0&date=2026-08-15&time=15:00&weather=sunny';
const LAKE = { lat: 46.7825, lon: 8.391, heading: 0, pitch: -35, mode: 'fly', height: 2400 };

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

test('the page draws the fixture with no WebGPU error, and the probe gives the height that an instance shader evaluates, to 1 mm', async ({ page }) => {
    // SwiftShader on the Linux runners spends its frames shading pixels: a quarter of them draws the same nodes.
    test.slow();
    await page.setViewportSize({ width: 480, height: 300 });
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

test('a ground worker keeps the ground bytes under a low alpha, and the colour under the snow mark', async ({ page }) => {
    // Lossless WebP (cwebp -lossless -exact): a 512 px ground tile of material 13, barren share 77 and lithology 5, region 0 in its
    // west half and 2 in its east half; a premultiplied decode loses the bytes under these alphas. And a quarter: an NDVI tile that
    // marks every texel with the snow mark, then the colour (90, 140, 60).
    const GROUND = 'UklGRjwAAABXRUJQVlA4TC8AAAAv/8F/EA8wTdOgIG0DFrY7IxAfBJm0zfyb3l0Fiej/BCx/zn/8x3/8x3/8x3+MBwA=';
    const QUARTER = 'UklGRhwAAABXRUJQVlA4TA8AAAAv/8A/AAfQqoj+ByKi/wEAUklGRi4AAABXRUJQVlA4TCIAAAAv/8F/AAdQxmqVp/8BgUCyv/cMRfQ/4z//+c9//vOf//wf';
    const worker = readdirSync(`${DIST}assets`).find(file => file.startsWith('ground-worker-'));
    await page.goto('/third-party-licenses.txt');
    const { probe, west, east } = await page.evaluate(async ({ worker, ground, quarter }) => {
        const body = base64 => Uint8Array.from(atob(base64), c => c.charCodeAt(0)).buffer;
        const tile = (key, base64, layer) => ({ key, z: 3, x: 5, y: 2, body: body(base64), layer });
        const job = { z: 3, x: 5, y: 2, ground: tile('ground', ground, 'ground'), quarters: [tile('q1', quarter, 'colour'), undefined], date: { weight: 0, snowIndex: 0, shares: [0.5, 0.5, 0.5] }, world: 9 };
        const builder = new Worker(`/assets/${worker}`, { type: 'module' });
        const ask = message => new Promise(resolve => { builder.onmessage = ({ data }) => resolve(data); builder.postMessage(message); });
        const probe = await ask({ id: 0 });
        const { pages: [nw, ne] } = await ask({ id: 1, job });
        return { probe, west: [...nw.cells.subarray(0, 4), ...nw.colour.subarray(0, 4), nw.snow[2]], east: [...ne.cells.subarray(0, 4)] };
    }, { worker, ground: GROUND, quarter: QUARTER });
    expect(probe).toEqual({ id: 0 });
    // The material, the lithology, the region and the barren share; then the colour, the NDVI code of a quarter that saw snow and the snow mark.
    expect(west).toEqual([13, 5, 0, 77, 90, 140, 60, 128, 255]);
    expect(east).toEqual([13, 5, 2, 77]);
});
