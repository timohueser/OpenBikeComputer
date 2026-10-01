import { expect, test } from '@playwright/test';
import { BASEMAP, basemapResponse } from './basemap-stub.js';

async function stubMap(page, fail = false) {
  const external = [];
  await page.route('**/*', (route) => {
    const url = new URL(route.request().url());
    if (url.hostname === '127.0.0.1') return route.continue();
    external.push(url.href);
    if (url.hostname !== BASEMAP) return route.abort();
    return fail ? route.fulfill({ status: 503 }) : route.fulfill(basemapResponse(url));
  });
  return external;
}

test('loads the shared basemap and changes its theme without public tiles', async ({ page }) => {
  const external = await stubMap(page);
  const light = page.waitForResponse(`https://${BASEMAP}/sprites/light.json`);
  await page.goto('/');
  await light;
  await expect(page.locator('.leaflet-gl-layer canvas')).toBeVisible();
  const dark = page.waitForResponse(`https://${BASEMAP}/sprites/dark.json`);
  await page.evaluate(() => { document.documentElement.dataset.theme = 'dark'; });
  await dark;
  await expect(page.locator('.basemap-warning')).toHaveCount(0);
  expect(external.length).toBeGreaterThan(0);
  expect(external.every((url) => new URL(url).hostname === BASEMAP)).toBe(true);
});

test('keeps selection available when the basemap is unreachable', async ({ page }) => {
  const external = await stubMap(page, true);
  await page.goto('/');
  await expect(page.getByRole('status').filter({ hasText: 'Map background unavailable.' })).toBeVisible();
  await expect(page.getByLabel('Search regions')).toBeVisible();
  await page.getByLabel('Search regions').fill('Bridge');
  await page.getByRole('button', { name: /^Add Bridge Fixture \(/ }).click();
  await expect(page.getByRole('button', { name: 'Bridge Fixture is already in the map' })).toBeVisible();
  expect(external.every((url) => new URL(url).hostname === BASEMAP)).toBe(true);
});

test('reports missing WebGL2 without requesting public tiles', async ({ page }) => {
  const external = await stubMap(page);
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await page.addInitScript(() => {
    const original = HTMLCanvasElement.prototype.getContext;
    HTMLCanvasElement.prototype.getContext = function (type, ...args) {
      return type === 'webgl2' ? null : original.call(this, type, ...args);
    };
  });
  await page.goto('/');
  await expect(page.locator('.basemap-warning')).toHaveText('Map background unavailable.');
  await expect(page.locator('.leaflet-gl-layer')).toHaveCount(0);
  expect(external).toEqual([]);
  expect(errors).toEqual([]);
});
