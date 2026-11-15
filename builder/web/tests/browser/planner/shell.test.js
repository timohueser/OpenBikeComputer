import { expect, test } from '@playwright/test';
import { CORS_PREFLIGHT, respond } from './stubs.js';

async function start(page, path) {
  // A GPU without an adapter: the TrailView window opens with its notice in a headless browser.
  await page.addInitScript(() => Object.defineProperty(Navigator.prototype, 'gpu', {
    configurable: true,
    get: () => ({ requestAdapter: async () => null, getPreferredCanvasFormat: () => 'bgra8unorm' }),
  }));
  await page.route(url => url.hostname !== '127.0.0.1', route => {
    if (route.request().method() === 'OPTIONS') return route.fulfill(CORS_PREFLIGHT);
    const response = respond(route.request());
    return response ? route.fulfill(response) : route.abort();
  });
  await page.goto(path);
  await expect(page.locator('.maplibregl-canvas')).toBeVisible();
  await expect(page.getByRole('navigation', { name: 'Pages' }).getByRole('link', { name: 'Planner' })).toHaveAttribute('aria-current', 'page');
}

test('a change of the hash closes an open window of the planner', async ({ page }) => {
  await start(page, '/planner.html#/planner');
  await page.getByRole('button', { name: 'Drag onto the map to open TrailView' }).click();
  const map = await page.locator('.maplibregl-canvas').boundingBox();
  await page.mouse.click(map.x + map.width / 2, map.y + map.height / 2);
  const view = page.getByRole('dialog', { name: 'TrailView' });
  await expect(view).toBeVisible();
  await page.evaluate(() => { location.hash = '#/device'; });
  await expect(view).toHaveCount(0);
  await expect(page.getByRole('navigation', { name: 'Pages' }).getByRole('link', { name: 'Device' })).toHaveAttribute('aria-current', 'page');
  await page.getByRole('link', { name: 'Planner' }).click();
  await expect(page.locator('.maplibregl-canvas')).toBeVisible();
});
