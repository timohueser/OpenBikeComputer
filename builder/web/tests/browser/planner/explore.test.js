import { expect, test } from '@playwright/test';
import { CORS_PREFLIGHT, respond } from './stubs.js';

// A town around the routes of the catalog in `stubs.js`.
const FREIBURG = { name: 'Freiburg', kind: 'city', lon: 7.85, lat: 47.99, west: 7.8, south: 47.96, east: 7.92, north: 48.02, region: 'Testland' };

test('the Explore page lists the signed routes of the map view, filters them, plans one and comes back to the same view', async ({ page }) => {
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.route(url => url.hostname !== '127.0.0.1', route => {
    const request = route.request();
    if (request.method() === 'OPTIONS') return route.fulfill(CORS_PREFLIGHT);
    if (request.url().endsWith('/query')) return route.fulfill({ ...CORS_PREFLIGHT, status: 200, json: { type: 'places', request: { type: 'place' }, results: [FREIBURG] } });
    const response = respond(request);
    return response ? route.fulfill(response) : route.abort();
  });

  // The whole area of the test build needs more catalog files than one view may load.
  await page.goto('/planner.html#/explore');
  const list = page.locator('.list');
  await expect(list.getByText('Zoom in to see the signed routes.')).toBeVisible();
  await expect(page.getByRole('link', { name: 'Explore' })).toHaveAttribute('aria-current', 'page');

  await page.getByRole('searchbox', { name: 'Find a place' }).fill('Freiburg');
  await page.getByRole('button', { name: /^Freiburg/ }).click();
  // The shortest first; the long route is no row, and the cycle route is not for hiking.
  await expect(list.getByText('2 signed routes in this map view')).toBeVisible();
  await expect(list.locator('.row')).toHaveText([/Schlossberg loop.*1\.2\skm · 150\sm up · T2/, /Westweg 2.*2\.5\skm · 80\sm up · T1/]);

  await page.getByText('+ Shape').click();
  await page.getByRole('button', { name: 'One way' }).click();
  await expect(list.getByText('1 of 2 signed routes')).toBeVisible();
  await list.getByRole('button', { name: /Westweg 2/ }).click();
  const card = page.getByRole('region', { name: 'Westweg 2' });
  await expect(card).toContainText('One way · Stage 2 of 2 · Westweg · National hiking route');
  const kept = await page.evaluate(() => new Promise(done => setTimeout(() => done(history.state.explore), 500)));

  await card.getByRole('button', { name: 'Plan this route' }).click();
  await expect(page).toHaveURL(/#\/planner$/);
  await expect(page.getByText(/Westweg 2 planned/)).toBeVisible();
  await page.getByRole('button', { name: 'Find another route' }).click();

  await expect(page).toHaveURL(/#\/explore$/);
  await expect(page.locator('.chip.set summary')).toHaveText(['Hiking ▾', 'One way ▾']);
  await expect(list.locator('.row[aria-current="true"]')).toContainText('Westweg 2');
  await expect(page.getByRole('region', { name: 'Westweg 2' })).toBeVisible();
  const back = await page.evaluate(() => new Promise(done => setTimeout(() => done(history.state.explore), 500)));
  expect(back.chips).toEqual(kept.chips);
  for (const [i, value] of kept.view.entries()) expect(back.view[i]).toBeCloseTo(value, 4);
  expect(errors).toEqual([]);
});
