import { expect, test } from '@playwright/test';
import { CORS_PREFLIGHT, respond } from './stubs.js';

const drawn = (id, kind, lon, from, to, extra = {}) => ({
  id, kind, label: id, coordinate: [lon, 47], ...extra,
  ...kind !== 'start' && { leg: 'drawn', drawn: [[lon - 0.05, 47, from], [lon - 0.025, 47, (from + to) / 2], [lon, 47, to]] },
});

// A plan of two days with drawn legs, so its line needs no routing request: 15.2 km along the parallel 47, the night in
// the middle.
const plan = {
  id: 'two', name: 'Two days', updatedAt: 0, summary: null,
  trip: {
    mode: 'trip', bike: 'touring', days: 2, daysSet: true, restAfter: [], routeOrder: ['up', 'night-1', 'flat'], points: [
      drawn('start', 'start', 8, 400, 400), drawn('up', 'waypoint', 8.05, 400, 600), drawn('night-1', 'night', 8.1, 600, 600, { night: 1, label: 'Inn' }),
      drawn('flat', 'waypoint', 8.15, 600, 600), drawn('finish', 'finish', 8.2, 600, 400),
    ],
  },
};
const CAFE = { name: 'Linde', kind: 'cafe', lon: 8.12, lat: 47.001, precision: 'place' };

test('the planner profile: a drag selects a part, a press on a day-end handle selects none, and Clear ends it', async ({ page }) => {
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  // The map fits the part at once, so it is at rest when the drag ends.
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.route(url => url.hostname !== '127.0.0.1', route => {
    const request = route.request();
    if (request.method() === 'OPTIONS') return route.fulfill(CORS_PREFLIGHT);
    if (request.url().endsWith('/query')) return route.fulfill({ ...CORS_PREFLIGHT, status: 200, json: { type: 'places', request: { type: 'places', what: ['cafe'] }, results: [CAFE] } });
    const response = respond(request);
    return response ? route.fulfill(response) : route.abort();
  });
  await page.goto('/planner.html?cover=none');
  await page.waitForFunction(() => window.coverTest);
  await page.evaluate(async record => {
    const { library } = window.coverTest;
    await library.saveRoute(record);
    await library.setState('active', record.id);
  }, plan);
  await page.goto('/planner.html#/planner');

  const profile = page.locator('.elevation');
  const plot = profile.locator('.plot');
  await expect(plot).toBeVisible();
  // The map reports its view once when it has loaded; that report would count as a move after the search.
  await expect(page.getByLabel('Route map')).toHaveAttribute('aria-busy', 'false', { timeout: 30_000 });
  await page.getByRole('textbox', { name: 'Find a place or ask about the route' }).fill('cafes');
  await page.keyboard.press('Enter');
  await expect(page.locator('.results')).toContainText('Linde');

  const drag = async (from, to) => {
    const box = await plot.boundingBox(), y = box.y + box.height / 2;
    await page.mouse.move(box.x + box.width * from, y);
    await page.mouse.down();
    await page.mouse.move(box.x + box.width * (from + to) / 2, y);
    await page.mouse.move(box.x + box.width * to, y);
    await page.mouse.up();
  };
  const part = profile.locator('.title .part');
  await profile.getByRole('radio', { name: 'Whole route' }).click();
  await drag(0.4, 0.6);
  await expect(part).toContainText(/^km\s6\.1 to km\s9\.1 · 3\.0\skm · \d+\sm up/);
  await expect(profile.locator('.pale')).toHaveCount(2);
  // The fit of the map to the part is no move of the map by the rider.
  const searchView = page.getByRole('button', { name: 'Search this map view' });
  await page.waitForTimeout(500);
  await expect(searchView).toHaveCount(0);
  await part.getByRole('button', { name: 'Clear' }).click();
  await expect(part).toHaveCount(0);

  // A move of the map by the rider offers the search of the new view.
  const map = await page.locator('.maplibregl-canvas').boundingBox();
  await page.mouse.move(map.x + map.width / 2, map.y + map.height / 3);
  await page.mouse.down();
  await page.mouse.move(map.x + map.width / 2 + 80, map.y + map.height / 3, { steps: 4 });
  await page.mouse.up();
  await expect(searchView).toBeVisible();

  // A drag that starts on the handle of the night selects no part.
  const handle = await profile.locator('.handle').boundingBox();
  await page.mouse.move(handle.x + handle.width / 2, handle.y + handle.height / 2);
  await page.mouse.down();
  await page.mouse.move(handle.x + handle.width / 2 + 30, handle.y + handle.height / 2);
  await page.mouse.move(handle.x + handle.width / 2 + 60, handle.y + handle.height / 2);
  await page.mouse.up();
  await expect(part).toHaveCount(0);
  await expect(profile.locator('.pale')).toHaveCount(0);
  expect(errors).toEqual([]);
});
