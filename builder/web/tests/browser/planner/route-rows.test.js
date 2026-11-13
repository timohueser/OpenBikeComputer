import { expect, test } from '@playwright/test';
import { CORS_PREFLIGHT, respond } from './stubs.js';

const drawn = (id, kind, lon, from, to, extra = {}) => ({
  id, kind, label: id, coordinate: [lon, 47], ...extra,
  ...kind !== 'start' && { leg: 'drawn', drawn: [[lon - 0.05, 47, from], [lon - 0.025, 47, (from + to) / 2], [lon, 47, to]] },
});

// A route of two days with drawn legs, so its line needs no routing request: 15.2 km along the parallel 47, a climb of
// 200 m to the first waypoint, the night in the middle.
const route = {
  id: 'two', revision: 0, name: 'Two days', updatedAt: 0,
  trip: {
    mode: 'trip', bike: 'touring', days: 2, daysSet: true, restAfter: [], routeOrder: ['up', 'night-1', 'flat'], points: [
      drawn('start', 'start', 8, 400, 400), drawn('up', 'waypoint', 8.05, 400, 600), drawn('night-1', 'night', 8.1, 600, 600, { night: 1, label: 'Inn' }),
      drawn('flat', 'waypoint', 8.15, 600, 600), drawn('finish', 'finish', 8.2, 600, 400),
    ],
  },
  summary: { line: [8, 47, 8.05, 47, 8.1, 47, 8.15, 47, 8.2, 47], heights: [400, 600, 600, 600, 400], bounds: [8, 47, 8.2, 47], lengthM: 15_184, climbM: 200, highestM: 600, seconds: 4000, bike: 'touring', days: 2 },
};

test('the rows of a route of two days: a day, the pointer, a climb, places and the state on the device', async ({ page }) => {
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.route(url => url.hostname !== '127.0.0.1', request => {
    if (request.request().method() === 'OPTIONS') return request.fulfill(CORS_PREFLIGHT);
    const response = respond(request.request());
    return response ? request.fulfill(response) : request.abort();
  });
  await page.goto('/planner.html#/route/two');
  await expect(page.getByText('This route is not in the library.')).toBeVisible();
  await expect(page.locator('.lanes')).toHaveCount(0);

  await page.goto('/planner.html?cover=none');
  await page.waitForFunction(() => window.coverTest);
  await page.evaluate(async record => {
    const { library, lineKey } = window.coverTest;
    const saved = await library.saveRoute(record);
    await library.changeState('sent', () => ({ two: { device: 'card:5', line: lineKey(saved) } }));
    localStorage.setItem('obc-device', JSON.stringify({ serial: 'A1B2', storeId: 'card', at: null, map: null, routes: 1, bytes: { map: 0, routes: 0, rides: 0, other: 0 }, came: null, skipped: [] }));
  }, route);
  await page.goto('/planner.html#/route/two');

  const lanes = page.locator('.lanes');
  for (const label of ['Waypoints', 'Elevation', 'Places']) await expect(lanes.locator('.label').filter({ hasText: label })).toBeVisible();
  await expect(lanes.locator('.ticks')).toContainText('start');
  await expect(lanes.locator('.ticks')).toContainText('15.2 km');
  await expect(lanes.locator('.pin')).toHaveText(['1', '2']);
  await expect(lanes.locator('.night')).toHaveCount(1);
  await expect(page.getByText('On the device')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Send to device' })).toBeDisabled();

  await page.getByRole('radio', { name: 'Day 2' }).click();
  await expect(lanes.locator('.pin')).toHaveText(['2']);
  await expect(lanes.locator('.ticks span').first()).toHaveText('Inn');

  const elevation = await lanes.locator('.lane').nth(1).boundingBox();
  // Day 2 starts at the night, at 7.6 km: a quarter of its width is 9.5 km from the start.
  await page.mouse.move(elevation.x + elevation.width * 0.25, elevation.y + elevation.height / 2);
  await expect(lanes.locator('.chip')).toContainText('9.5 km');

  await page.getByRole('radio', { name: 'Both days' }).click();
  const mark = page.locator('.route-map-mark');
  await page.mouse.move(elevation.x + elevation.width * 0.25, elevation.y + elevation.height / 2);
  await expect(mark).toBeVisible();
  await expect(lanes.locator('.chip')).toContainText('3.8 km');
  const first = await mark.boundingBox();
  await page.mouse.move(elevation.x + elevation.width * 0.75, elevation.y + elevation.height / 2);
  await expect.poll(async () => (await mark.boundingBox()).x).toBeGreaterThan(first.x + 20);

  // The pins of the map are icons; a click shows the name, Escape hides it.
  const mapPins = page.locator('.route-map .route-pin');
  await expect(mapPins.locator('button')).toHaveText(['1', '2', '']);
  await expect(mapPins.locator('.route-pin-label')).toHaveCount(0);
  await mapPins.getByRole('button', { name: 'up' }).click();
  await expect(mapPins.locator('.route-pin-label')).toHaveText('up');
  await page.keyboard.press('Escape');
  await expect(mapPins.locator('.route-pin-label')).toHaveCount(0);

  await page.getByRole('combobox', { name: 'Places' }).selectOption('water');
  await expect(lanes).toContainText('None within 1.0 km');

  await page.locator('.facts .more').click();
  await page.locator('.facts .climb button').first().click();
  await expect(page.getByRole('button', { name: '‹ Climbs' })).toBeVisible();
  await expect(page.locator('.facts dl > div').filter({ hasText: 'Steepest' })).toContainText('5 %');
  await expect(page.locator('.facts')).toContainText('Steepest 500 m');

  await page.evaluate(() => new Promise(resolve => {
    const open = indexedDB.open('obc-library');
    open.onsuccess = () => {
      const transaction = open.result.transaction('state', 'readwrite');
      transaction.objectStore('state').put({ two: { device: 'card:5', line: 'another' } }, 'sent');
      transaction.oncomplete = resolve;
    };
  }));
  await page.reload();
  await expect(page.getByText('Changed since it was sent')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Send to device' })).toBeEnabled();
  expect(errors).toEqual([]);
});
