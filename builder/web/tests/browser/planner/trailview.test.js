import { expect, test } from '@playwright/test';
import { CORS_PREFLIGHT, respond } from './stubs.js';

test('Esc stops a drag of the binoculars, a drop on the map opens TrailView, and Esc closes it', async ({ page }) => {
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
  const dragOntoMap = async () => {
    await page.mouse.move(from.x + from.width / 2, from.y + from.height / 2);
    await page.mouse.down();
    await page.mouse.move(to.x + to.width / 2, to.y + to.height / 2, { steps: 12 });
  };

  // Esc in a drag ends only the drag: the callout of the planner stays open.
  await page.mouse.click(to.x + to.width * 0.3, to.y + to.height * 0.6);
  const callout = page.getByRole('dialog', { name: 'Map details' });
  await expect(callout).toBeVisible();
  await dragOntoMap();
  await page.keyboard.press('Escape');
  await page.mouse.up();
  await expect(callout).toBeVisible();

  await dragOntoMap();
  await page.mouse.up();

  const view = page.getByRole('dialog', { name: 'TrailView' });
  await expect(view).toBeVisible();
  await expect(view.getByRole('alert')).toHaveText('TrailView cannot use the graphics card of this device.');
  await expect(view.getByText('Terrain test data')).toBeHidden();
  await view.getByRole('button', { name: 'Credits' }).hover();
  await expect(view.getByText('Terrain test data')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(view).toHaveCount(0);
  await expect(binoculars).toBeFocused();
  expect(errors).toEqual([]);
});
