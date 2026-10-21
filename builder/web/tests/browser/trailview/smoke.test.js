import { expect, test } from '@playwright/test';
import { errorsOf, redPixels, routeFixture } from './harness.js';

// A walking eye that looks down: few pages to synthesize, which SwiftShader does on the CPU, twice in a test.
const WALK = '?lat=46.820500&lon=8.403000&heading=160.0&pitch=-45.0&eye=1.6&date=2026-08-15&time=15:00&weather=sunny';
// Down onto the middle of the fixture, so that only known terrain fills the frame and several levels meet.
const DOWN = '?lat=46.797500&lon=8.415000&heading=20.0&pitch=-60.0&alt=2500.0&date=2026-08-15&time=15:00&weather=sunny';

async function open(page, view) {
    await routeFixture(page);
    await page.goto(`/trailview.html${view}`);
    await page.waitForFunction(() => window.trailView, null, { timeout: 60_000 });
    await page.evaluate(() => window.trailView.waitForIdle());
}

test('the page draws the fixture with no WebGPU error, and a lost device recovers without a request', async ({ page }) => {
    const errors = errorsOf(page);
    await open(page, WALK);
    const before = await page.evaluate(() => window.trailView.stats());
    expect(before.drawn).toBeGreaterThan(0);
    expect(before.failed).toBe(0);

    await page.evaluate(() => window.trailView.gpu.device.destroy());
    await page.waitForFunction(() => window.trailView.recoveries === 1);
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

test('a browser without WebGPU shows a notice', async ({ page }) => {
    await page.addInitScript(() => Object.defineProperty(Navigator.prototype, 'gpu', { get: () => undefined }));
    await routeFixture(page);
    await page.goto('/trailview.html');
    await expect(page.getByRole('alert')).toHaveText(/needs WebGPU/);
});
