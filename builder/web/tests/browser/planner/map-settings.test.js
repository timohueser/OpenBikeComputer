import { expect, test } from '@playwright/test';
import { CORS_PREFLIGHT, respond } from './stubs.js';

// The three snow archives as the tile service gives them: TileJSON with the archive metadata, and no tiles.
const SNOW = {
  tilejson: '3.0.0', tiles: ['https://tiles.test/snow/{z}/{x}/{y}'], minzoom: 0, maxzoom: 7, bounds: [7.45, 47.5, 10.5, 49.85],
  snow_format: 1, first_season: 2001, seasons: 20, resolution_m: 500, attribution: 'Snow test data',
};

const overlap = (a, b) => a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;
const inside = (inner, outer) => inner.x >= outer.x && inner.y >= outer.y && inner.x + inner.width <= outer.x + outer.width && inner.y + inner.height <= outer.y + outer.height;

test('the map settings show their pictures, and the legend of a data layer stands on the map above the date bar', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.route(url => url.hostname !== '127.0.0.1', route => {
    const request = route.request();
    if (request.method() === 'OPTIONS') return route.fulfill(CORS_PREFLIGHT);
    const { pathname } = new URL(request.url());
    if (/^\/snow(-levels|-seasons)?\.json$/.test(pathname)) return route.fulfill({ headers: CORS_PREFLIGHT.headers, json: SNOW });
    if (pathname.startsWith('/snow/')) return route.fulfill({ headers: CORS_PREFLIGHT.headers, status: 204 });
    const response = respond(request);
    return response ? route.fulfill(response) : route.abort();
  });
  await page.goto('/planner.html#/planner');
  await page.getByRole('button', { name: 'Map settings' }).click();
  const popup = page.getByRole('dialog', { name: 'Map settings' });
  const pictures = popup.locator('img');
  await expect(pictures).toHaveCount(9);
  await expect.poll(() => pictures.evaluateAll(images => images.filter(image => !image.complete || !image.naturalWidth).length)).toBe(0);

  await popup.getByRole('button', { name: 'Snow', exact: true }).click();
  const legend = page.getByRole('img', { name: /^Snow colours/ });
  const dateBar = page.getByRole('group', { name: 'Layer date' });
  await expect(legend).toBeVisible();
  await expect(dateBar).toBeVisible();
  await expect(popup.getByRole('img', { name: /^Snow colours/ })).toHaveCount(0);
  const [map, legendBox, dateBox] = await Promise.all([page.locator('.map-area'), legend, dateBar].map(part => part.boundingBox()));
  expect(inside(legendBox, map) && inside(dateBox, map)).toBe(true);
  expect(overlap(legendBox, dateBox)).toBe(false);
});
