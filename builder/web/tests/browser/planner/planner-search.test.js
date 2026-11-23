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

// A plan of two days with drawn legs, so its line needs no routing request: 15.2 km along the parallel 47.
const PLAN = {
  id: 'two', name: 'Two days', updatedAt: 0, summary: null,
  trip: { mode: 'trip', bike: 'touring', days: 2, daysSet: true, routeOrder: ['night-1'], points: [
    { id: 'start', kind: 'start', label: 'Freiburg', coordinate: [8, 47] },
    { id: 'night-1', kind: 'night', night: 1, label: 'Inn', coordinate: [8.1, 47], leg: 'drawn', drawn: [[8.05, 47, 500], [8.1, 47, 600]] },
    { id: 'finish', kind: 'finish', label: 'Konstanz', coordinate: [8.2, 47], leg: 'drawn', drawn: [[8.15, 47, 500], [8.2, 47, 400]] },
  ] },
};
/** Supermarkets at km 2, 5 and 9 of the plan. */
const SHOPS = [2, 5, 9].map(km => ({ source: `n/${km}`, name: `Markt ${km}`, kind: 'supermarket', lon: 8 + km / 75.9, lat: 47.001, precision: 'place',
  distance: 0.1, position: { along: km, distance: 0.1 } }));

test('the planner search: "route to the next supermarket" lists the supermarkets ahead, the amber button routes to the first, and "Undo" brings the plan back', async ({ page, context }) => {
  const errors = [], sent = [];
  page.on('pageerror', error => errors.push(error.message));
  await context.grantPermissions(['geolocation']);
  await context.setGeolocation({ latitude: 47.001, longitude: 8.005 });
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.route(url => url.hostname !== '127.0.0.1', route => {
    const request = route.request();
    if (request.method() === 'OPTIONS') return route.fulfill(CORS_PREFLIGHT);
    if (request.url().endsWith('/query')) {
      const input = request.postDataJSON(), asked = input.request ?? { type: 'route', to: { kind: 'supermarket' } };
      sent.push(asked);
      // As the server: the place of the rider is asked for once a request needs it.
      const json = asked.type === 'route' ? { type: 'change', request: asked, description: 'Route from Your location to Markt 2', changes: [] }
        : asked.where?.along?.ref === 'here' && !input.here ? { type: 'unresolved', code: 'location', request: asked, results: [] }
        : { type: 'places', request: asked, results: SHOPS, area: 'Ahead of you' };
      return route.fulfill({ ...CORS_PREFLIGHT, status: 200, json });
    }
    const response = respond(request);
    return response ? route.fulfill(response) : route.abort();
  });
  await page.goto('/planner.html?cover=none');
  await page.waitForFunction(() => window.coverTest);
  await page.evaluate(async record => {
    const { library } = window.coverTest;
    await library.saveRoute(record);
    await library.setState('active', record.id);
  }, PLAN);
  await page.goto('/planner.html#/planner');
  await expect(page.locator('main')).toHaveAttribute('aria-busy', 'false');
  const field = page.getByRole('combobox', { name: 'Find a place or ask about the route' });
  const distance = page.locator('[aria-label="Whole plan"] dd').first();
  await expect(distance).toHaveText('15.2 km');

  // The action marks as the action, not as "Whole route"; "next" marks as the nearest one ahead once a kind follows it.
  await field.pressSequentially('route to the next supermarket');
  await expect(page.locator('.mirror .m-value')).toHaveText(['route to']);
  await field.press('Enter');
  await expect(page.locator('[data-slot="find"] .label')).toHaveText('Route to');
  await expect(page.locator('[data-slot="find"] strong')).toHaveText('Supermarket');
  await expect(page.locator('[data-slot="where"] strong')).toHaveText('Nearest to me');
  const near = { along: { ref: 'here', from: { value: 0, unit: 'km' }, to: { value: 20, unit: 'km' } } };
  expect(sent.at(-1)).toEqual({ type: 'places', what: ['supermarket'], where: near });

  // One amber button, on the first place; nothing changes the plan before a press. Escape closes the values that riders
  // often add next, which cover the list.
  await field.press('Escape');
  const actions = page.locator('.result .act');
  await expect(actions).toHaveText(['Route here', 'Route here', 'Route here']);
  await expect(page.locator('.result .act.primary')).toHaveCount(1);
  await expect(actions.first()).toHaveClass(/primary/);
  await expect(distance).toHaveText('15.2 km');
  await actions.first().click();
  await expect(page.locator('.done')).toContainText('The route now ends at Markt 2');
  await expect(distance).not.toHaveText('15.2 km');
  // The list stays, also when the plan of two days becomes a route of one; "Undo" undoes the change of the plan and takes
  // the line away.
  await expect(actions).toHaveCount(3);
  await page.locator('.done').getByRole('button', { name: 'Undo' }).click();
  await expect(distance).toHaveText('15.2 km');
  await expect(page.locator('.done')).toHaveCount(0);
  await expect(actions).toHaveCount(3);
  expect(errors).toEqual([]);
});
