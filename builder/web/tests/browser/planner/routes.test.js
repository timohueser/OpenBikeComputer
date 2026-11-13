import { expect, test } from '@playwright/test';
import { CORS_PREFLIGHT, respond } from './stubs.js';

const drawn = (id, kind, lat, km) => ({
  id, kind, label: id, coordinate: [8, lat],
  ...kind !== 'start' && { leg: 'drawn', drawn: [[8, lat - km / 222, 400], [8, lat, 500]] },
});
const summary = (lengthM, days) => ({ line: [8, 48, 8, 48.2], heights: [300, 700], bounds: [8, 48, 8, 48.2], lengthM, climbM: 400, highestM: 700, seconds: 4000, bike: 'touring', days });
const trip = days => ({ mode: 'route', bike: 'touring', days, routeOrder: [], points: [drawn('Start', 'start', 48), drawn('Finish', 'finish', 48.2, 22)] });

// A named route that waits, a plan with no name on the device with an older line, and a route that is not a plan.
const routes = [
  { id: 'lake', revision: 0, name: 'Lake test', updatedAt: 3, trip: trip(1), summary: summary(22_239, 1) },
  { id: 'long', revision: 0, name: '', updatedAt: 2, trip: trip(3), summary: summary(150_000, 3) },
  { id: 'broken', revision: 0, name: '', updatedAt: 1, trip: { nope: true }, summary: summary(22_239, 1) },
];

test('the Routes page lists the routes, filters them with a chip, shows the selected one and its state', async ({ page }) => {
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.route(url => url.hostname !== '127.0.0.1', request => {
    if (request.request().method() === 'OPTIONS') return request.fulfill(CORS_PREFLIGHT);
    const response = respond(request.request());
    return response ? request.fulfill(response) : request.abort();
  });
  await page.goto('/planner.html#/routes');
  await expect(page.getByText('No routes yet.')).toBeVisible();

  await page.goto('/planner.html?cover=none');
  await page.waitForFunction(() => window.coverTest);
  await page.evaluate(async records => {
    const { library } = window.coverTest;
    for (const record of records) await library.saveRoute(record);
    await library.setState('waiting', ['lake']);
    await library.setState('sent', { long: { device: 'card:7', key: 'older' } });
    localStorage.setItem('obc-device', JSON.stringify({ serial: '00AB', storeId: 'card', at: null, map: null, routes: 1, bytes: { map: 0, routes: 0, rides: 0, other: 0 }, came: null, skipped: [] }));
  }, routes);

  await page.goto('/planner.html#/routes');
  const list = page.locator('ul').filter({ hasText: 'Lake test' });
  await expect(page.getByText('3 routes')).toBeVisible();
  await expect(list.getByRole('button')).toHaveText([/Lake test.*Waits/, /Start → Finish.*3 days · 150\skm.*Changed/, /Route that cannot be read/]);

  await list.getByRole('button', { name: 'Route that cannot be read' }).click();
  const card = page.getByRole('region', { name: 'Route that cannot be read' });
  await expect(card.getByRole('link', { name: 'Route that cannot be read' })).toHaveAttribute('href', '#/route/broken');
  await expect(card.getByRole('button')).toHaveCount(0);

  await list.getByRole('button', { name: /Start → Finish/ }).click();
  const plan = page.getByRole('region', { name: 'Start → Finish' });
  await expect(plan).toContainText('Changed since it was sent');
  await plan.getByRole('button', { name: 'Send to device' }).click();
  await expect(plan).toContainText('Waits for the device');
  await expect(list.getByRole('button', { name: /Start → Finish/ })).toContainText('Waits');
  await plan.getByRole('button', { name: 'Do not send' }).click();
  await expect(plan).toContainText('Changed since it was sent');

  // A click anywhere on the card opens the route; a click on the empty map and Escape close the card.
  // The name is the link that covers the card: a click on the card text lands on it, as a click of the rider does.
  await plan.getByText('Touring bike · 3 days').click({ force: true });
  await expect(page).toHaveURL(/#\/route\/long$/);
  await page.goBack();
  const row = list.getByRole('button', { name: /Start → Finish/ });
  await row.click();
  const map = page.locator('.map');
  await expect(map.locator('.maplibregl-canvas')).toBeVisible();
  const box = await map.boundingBox();
  await page.mouse.click(box.x + 40, box.y + box.height / 2);
  await expect(plan).toHaveCount(0);
  await row.click();
  await page.keyboard.press('Escape');
  await expect(plan).toHaveCount(0);

  await page.getByText('+ Length').click();
  await page.getByRole('button', { name: 'Under 100 km' }).click();
  await expect(page.getByText('1 of 3 routes')).toBeVisible();
  await expect(list.getByRole('button')).toHaveText([/Lake test/]);
  await expect(plan).toHaveCount(0);

  // Back from a route brings the search again.
  await list.getByRole('button', { name: /Lake test/ }).click();
  await page.getByRole('region', { name: 'Lake test' }).getByRole('link', { name: 'Lake test' }).click();
  await expect(page.getByRole('heading', { name: 'Lake test', level: 1 })).toBeVisible();
  await page.goBack();
  await expect(page.getByText('1 of 3 routes')).toBeVisible();

  await page.getByRole('link', { name: 'New route' }).click();
  await expect(page).toHaveURL(/#\/planner$/);
  await expect(page.locator('.maplibregl-canvas').first()).toBeVisible();
  expect(errors).toEqual([]);
});
