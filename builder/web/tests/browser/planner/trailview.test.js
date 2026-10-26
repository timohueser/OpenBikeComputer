import { expect, test } from '@playwright/test';
import { CORS_PREFLIGHT, respond } from './stubs.js';

test('the binoculars dragged onto the map open TrailView, and Esc closes it', async ({ page }) => {
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  // A GPU without an adapter: the window shows its notice, so the journey does not depend on WebGPU in a headless browser.
  await page.addInitScript(() => Object.defineProperty(Navigator.prototype, 'gpu', {
    configurable: true,
    get: () => ({ requestAdapter: async () => null, getPreferredCanvasFormat: () => 'bgra8unorm' }),
  }));
  await page.route(url => url.hostname !== '127.0.0.1', route => {
    if (route.request().method() === 'OPTIONS') return route.fulfill(CORS_PREFLIGHT);
    const response = respond(route.request());
    return response ? route.fulfill(response) : route.abort();
  });
  await page.goto('/planner.html');
  const map = page.locator('.maplibregl-canvas');
  await expect(map).toBeVisible();

  const binoculars = page.getByRole('button', { name: 'Drag onto the map to open TrailView' });
  const from = await binoculars.boundingBox();
  const to = await map.boundingBox();
  await page.mouse.move(from.x + from.width / 2, from.y + from.height / 2);
  await page.mouse.down();
  await page.mouse.move(to.x + to.width / 2, to.y + to.height / 2, { steps: 12 });
  await expect(page.locator('.area-frame')).toBeVisible();
  await page.mouse.up();

  const view = page.getByRole('dialog', { name: 'TrailView' });
  await expect(view).toBeVisible();
  await expect(view.getByRole('alert')).toHaveText('TrailView needs WebGPU. Open the planner in a current version of Chrome, Edge or Safari.');
  await expect(view).toContainText('Terrain test data');
  await expect(page.locator('.area-frame')).toHaveCount(0);
  await page.keyboard.press('Escape');
  await expect(view).toHaveCount(0);
  await expect(binoculars).toBeFocused();
  expect(errors).toEqual([]);
});
