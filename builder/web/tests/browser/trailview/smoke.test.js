import { expect, test } from '@playwright/test';
import { errorsOf, routeFixture } from './harness.js';

const VIEW = '?lat=46.820500&lon=8.403000&heading=160.0&pitch=5.0&eye=1.6&date=2026-08-15&time=15:00&weather=sunny';

test('the page draws the fixture with no WebGPU error, and a lost device recovers without a request', async ({ page }) => {
    const errors = errorsOf(page);
    await routeFixture(page);
    await page.goto(`/trailview.html${VIEW}`);
    await page.waitForFunction(() => window.trailView, null, { timeout: 60_000 });
    await page.evaluate(() => window.trailView.waitForIdle());
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

test('a browser without WebGPU shows a notice', async ({ page }) => {
    await page.addInitScript(() => Object.defineProperty(Navigator.prototype, 'gpu', { get: () => undefined }));
    await routeFixture(page);
    await page.goto('/trailview.html');
    await expect(page.getByRole('alert')).toHaveText(/needs WebGPU/);
});
