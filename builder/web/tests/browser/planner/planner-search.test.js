import { expect, test } from '@playwright/test';
import { CORS_PREFLIGHT, respond } from './stubs.js';

const FOUNTAIN = { source: 'n/1', name: 'Brunnen am Markt', kind: 'drinking_water', lon: 8.12, lat: 47.001, precision: 'place', distance: 1 };
/** The request that the query model reads in the sentence of this test. */
const READ = { 'drinking water in the middle of day 1': { type: 'places', what: ['drinking_water'], where: { day: 1, part: 'middle' } } };

test('the planner search: a sentence marks its words, and Enter puts what the model read in Find, Where and When', async ({ page }) => {
  const errors = [], sent = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.route(url => url.hostname !== '127.0.0.1', route => {
    const request = route.request();
    if (request.method() === 'OPTIONS') return route.fulfill(CORS_PREFLIGHT);
    if (request.url().endsWith('/query')) {
      const input = request.postDataJSON();
      sent.push([input.q, input.request]);
      return route.fulfill({ ...CORS_PREFLIGHT, status: 200, json: { type: 'places', request: input.request ?? READ[input.q], results: [FOUNTAIN] } });
    }
    const response = respond(request);
    return response ? route.fulfill(response) : route.abort();
  });
  await page.goto('/planner.html#/planner');
  // The side panel takes no input until the planner has loaded.
  await expect(page.locator('main')).toHaveAttribute('aria-busy', 'false');
  const field = page.getByRole('combobox', { name: 'Find a place or ask about the route' });
  const slot = id => page.locator(`[data-slot="${id}"] strong`);

  // The words stay in the field while the rider types; the words in progress get their mark at Enter.
  await field.pressSequentially('drinking water in the middle of day 1');
  await expect(page.locator('.mirror .m-value')).toHaveText(['drinking water', 'middle']);
  await field.press('Enter');
  await expect(slot('find')).toHaveText('Drinking water');
  await expect(slot('where')).toHaveText('Middle of Day 1');
  await expect(slot('when')).toHaveText('Any time');
  await expect(field).toHaveValue('');
  await expect(page.locator('.results')).toContainText('Brunnen am Markt');

  // A sentence of only an area or a time changes its slot with no model, and so does a panel.
  await field.fill('open now');
  await field.press('Enter');
  await expect(slot('when')).toHaveText('Open now');
  await page.locator('[data-slot="find"]').click();
  await page.getByRole('button', { name: 'Food', exact: true }).click();
  await expect(slot('find')).toHaveText('Food');
  const where = { day: 1, part: 'middle' }, now = { now: true };
  await expect.poll(() => sent).toEqual([
    ['drinking water in the middle of day 1', undefined],
    ['drinking water in the middle of day 1', { type: 'places', what: ['drinking_water'], where, open: now }],
    ['drinking water in the middle of day 1', { type: 'places', what: ['food'], where, open: now }],
  ]);
  // In a short window the open panel ends over the bottom of the page. The panel of "Find" is wider than the side panel.
  await page.setViewportSize({ width: 1280, height: 600 });
  const panel = page.locator('#search-panel');
  await expect(panel.getByRole('button', { name: 'Done' })).toBeInViewport({ ratio: 1 });
  expect((await panel.boundingBox()).width).toBeGreaterThan((await page.locator('.search-bar').boundingBox()).width);
  expect(errors).toEqual([]);
});
