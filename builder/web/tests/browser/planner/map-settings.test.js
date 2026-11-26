import { expect, test } from '@playwright/test';
import { CORS_PREFLIGHT, respond } from './stubs.js';

// The climate archive as the tile service gives it: TileJSON with the archive metadata, and no tiles.
const CLIMATE = {
  tilejson: '3.0.0', tiles: ['https://tiles.test/climate/{z}/{x}/{y}'], minzoom: 0, maxzoom: 7, bounds: [7.45, 47.5, 10.5, 49.85],
  first_year: 2015, years: 10, wet_day_mm: 1, attribution: 'Climate test data',
};

const overlap = (a, b) => a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;
const inside = (inner, outer) => inner.x >= outer.x && inner.y >= outer.y && inner.x + inner.width <= outer.x + outer.width && inner.y + inner.height <= outer.y + outer.height;

test('the map settings show their pictures, and the legend of a data layer stands on the map above the date bar', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.route(url => url.hostname !== '127.0.0.1', route => {
    const request = route.request();
    if (request.method() === 'OPTIONS') return route.fulfill(CORS_PREFLIGHT);
    const { pathname } = new URL(request.url());
    if (pathname === '/climate.json') return route.fulfill({ headers: CORS_PREFLIGHT.headers, json: CLIMATE });
    if (pathname.startsWith('/climate/')) return route.fulfill({ headers: CORS_PREFLIGHT.headers, status: 204 });
    const response = respond(request);
    return response ? route.fulfill(response) : route.abort();
  });
  await page.goto('/planner.html#/planner');
  await page.getByRole('button', { name: 'Map settings' }).click();
  const popup = page.getByRole('dialog', { name: 'Map settings' });
  const pictures = popup.locator('img');
  await expect(pictures).toHaveCount(9);
  await expect.poll(() => pictures.evaluateAll(images => images.filter(image => !image.complete || !image.naturalWidth).length)).toBe(0);

  // Wind has the widest legend.
  await popup.getByRole('button', { name: 'Wind', exact: true }).click();
  const legend = page.getByRole('status').filter({ has: page.getByRole('img', { name: /^Wind colours/ }) });
  const dateBar = page.getByRole('group', { name: 'Layer date' });
  await expect(legend).toBeVisible();
  await expect(dateBar).toBeVisible();
  await expect(popup.getByRole('img', { name: /^Wind colours/ })).toHaveCount(0);
  const [map, legendBox, dateBox] = await Promise.all([page.locator('.map-area'), legend, dateBar].map(part => part.boundingBox()));
  expect(inside(legendBox, map) && inside(dateBox, map)).toBe(true);
  expect(overlap(legendBox, dateBox)).toBe(false);
  expect(legendBox.x >= dateBox.x && legendBox.x + legendBox.width <= dateBox.x + dateBox.width).toBe(true);
});
