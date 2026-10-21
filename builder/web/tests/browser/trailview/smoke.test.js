import { readdirSync } from 'node:fs';
import { expect, test } from '@playwright/test';
import { DIST, errorsOf, redPixels, routeFixture } from './harness.js';

// A walking eye that looks down: few pages to synthesize, which SwiftShader does on the CPU, twice in a test.
const WALK = '?lat=46.820500&lon=8.403000&heading=160.0&pitch=-45.0&eye=1.6&date=2026-08-15&time=15:00&weather=sunny';
// Down onto the middle of the fixture, so that only known terrain fills the frame and several levels meet.
const DOWN = '?lat=46.797500&lon=8.415000&heading=20.0&pitch=-60.0&alt=2500.0&date=2026-08-15&time=15:00&weather=sunny';
// Straight down onto the roads of Engelberg from 100 m, where the finest level shows them, and onto Trübsee from 2 km.
const VILLAGE = '?lat=46.818000&lon=8.405000&heading=0.0&pitch=-89.0&alt=1100.0&date=2026-08-15&time=15:00&weather=sunny';
const LAKE = '?lat=46.782500&lon=8.391000&heading=0.0&pitch=-35.0&alt=2400.0&date=2026-08-15&time=15:00&weather=sunny';

async function open(page, view) {
    await routeFixture(page);
    await page.goto(`/trailview.html${view}`);
    await page.waitForFunction(() => window.trailView, null, { timeout: 60_000 });
    await page.evaluate(() => window.trailView.waitForIdle());
}

test('the page draws the fixture with no WebGPU error, and a lost device recovers without a request', async ({ page }) => {
    // SwiftShader loads, synthesizes and stamps the walk twice.
    test.slow();
    const errors = errorsOf(page);
    await open(page, WALK);
    const before = await page.evaluate(() => window.trailView.stats());
    expect(before.drawn).toBeGreaterThan(0);
    expect(before.failed).toBe(0);

    await page.evaluate(() => window.trailView.gpu.device.destroy());
    // SwiftShader compiles every pipeline again, which takes it some 20 s.
    await page.waitForFunction(() => window.trailView.recoveries === 1, null, { timeout: 60_000 });
    await page.evaluate(() => window.trailView.waitForIdle());
    const after = await page.evaluate(() => window.trailView.stats());
    expect(after.drawn).toBe(before.drawn);
    expect(after.requests).toBe(before.requests);
    expect(errors).toEqual([]);
});

test('the terrain has no crack: no pixel shows the red clear colour', { tag: '@gpu' }, async ({ page }) => {
    const errors = errorsOf(page);
    await open(page, `${DOWN}&debug=cracks`);
    await page.addStyleTag({ content: '#hud, #credits { display: none; }' });
    const { drawn, terrain } = await page.evaluate(() => window.trailView.stats());
    expect(drawn).toBeGreaterThan(0);
    expect(terrain.resident.synth).toBeGreaterThan(0);
    expect(await redPixels(page)).toBe(0);
    expect(errors).toEqual([]);
});

test('drawnHeight is the drawn mesh: no pixel leaves it by 5 cm', { tag: '@gpu' }, async ({ page }) => {
    const errors = errorsOf(page);
    await open(page, DOWN);
    await page.addStyleTag({ content: '#hud, #credits { display: none; }' });
    const { terrain } = await page.evaluate(() => window.trailView.stats());
    expect(terrain.resident.synth).toBeGreaterThan(0);
    // The surface view evaluates drawnHeight per pixel: one frame of it, once the pages are idle.
    await page.evaluate(async () => {
        const tv = window.trailView;
        tv.view = { ...tv.view, debug: 'surface' };
        await tv.nextFrame();
    });
    expect(await redPixels(page)).toBe(0);
    expect(errors).toEqual([]);
});

test('the probe gives the height that an instance shader evaluates, to 1 mm', async ({ page }) => {
    await open(page, DOWN);
    const points = Array.from({ length: 16 }, (_, i) => [8.415 + 0.002 * Math.cos(i), 46.7975 + 0.0015 * Math.sin(2 * i)]);
    const { probe, instance } = await page.evaluate(points => window.trailView.check(points), points);
    expect(probe.every(Number.isFinite)).toBe(true);
    for (const [i, height] of probe.entries()) expect(Math.abs(height - instance[i])).toBeLessThan(1e-3);
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

test('the stamps hold along the roads of a village, and debug=mask draws them', { tag: '@gpu' }, async ({ page }) => {
    const errors = errorsOf(page);
    await open(page, VILLAGE);
    const audit = await audited(page);
    expect(audit.checks.crossFall.checked).toBeGreaterThan(100);
    expect(audit.checks.roadHeight.checked).toBeGreaterThan(100);
    await page.evaluate(async () => {
        const tv = window.trailView;
        tv.view = { ...tv.view, debug: 'mask' };
        await tv.nextFrame();
    });
    expect((await page.evaluate(() => window.trailView.stats())).terrain.masks).toBeGreaterThan(0);
    expect(errors).toEqual([]);
});

test('the stamps hold in a lake at every level that the frame draws, at its one level', { tag: '@gpu' }, async ({ page }) => {
    const errors = errorsOf(page);
    await open(page, LAKE);
    const audit = await audited(page);
    expect(audit.checks.inWater.checked).toBeGreaterThan(50);
    expect(audit.checks.onBank.checked).toBeGreaterThan(0);
    expect(audit.levels.lakes).toBeGreaterThan(0);
    expect(errors).toEqual([]);
});

test('a ground worker keeps the colour under the snow mark of a colour tile', async ({ page }) => {
    // Lossless WebP (cwebp -lossless -exact): a 512 px material tile of code 13, and a 64 px quarter of
    // colour (90, 140, 60) with alpha 1, the snow mark, which a premultiplied decode turns into (0, 255, 0).
    const MATERIAL = 'UklGRiwAAABXRUJQVlA4TCAAAAAv/8F/AAcQbYj+BwQCyf7eMxTR/4z//Oc///nPf/7zfw==';
    const QUARTER = 'UklGRiIAAABXRUJQVlA4TBYAAAAvP8APEAdQxmqVJwNAQvj/Xovof+oH';
    const worker = readdirSync(`${DIST}assets`).find(file => file.startsWith('ground-worker-'));
    await page.goto('/third-party-licenses.txt');
    const texel = await page.evaluate(async ({ worker, material, quarter }) => {
        const body = base64 => Uint8Array.from(atob(base64), c => c.charCodeAt(0)).buffer;
        const tile = (key, base64) => ({ key, z: 3, x: 5, y: 2, body: body(base64) });
        const job = { z: 3, x: 5, y: 2, material: tile('material', material), quarters: [tile('q1', quarter), undefined], seasons: 0, date: { weight: 0, snowIndex: 0, shares: [0.5] }, world: 0 };
        const ground = new Worker(`/assets/${worker}`, { type: 'module' });
        const { data } = await new Promise(resolve => { ground.onmessage = resolve; ground.postMessage({ id: 0, job }); });
        return [...data.pages[0].colour.subarray(0, 4), data.pages[0].snow[2]];
    }, { worker, material: MATERIAL, quarter: QUARTER });
    // The colour, the NDVI code of a quarter that saw snow, and the snow mark.
    expect(texel).toEqual([90, 140, 60, 128, 255]);
});

test('a browser without WebGPU shows a notice', async ({ page }) => {
    await page.addInitScript(() => Object.defineProperty(Navigator.prototype, 'gpu', { get: () => undefined }));
    await routeFixture(page);
    await page.goto('/trailview.html');
    await expect(page.getByRole('alert')).toHaveText(/needs WebGPU/);
});
