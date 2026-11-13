import { expect, test } from '@playwright/test';
import { PbfWriter } from 'pbf';
import { CORS_PREFLIGHT, respond } from './stubs.js';

/** A vector tile with a layer `pois` of restaurants: each place is a point with its kind, name and coordinate. */
function placeTile(places) {
  const pbf = new PbfWriter(), keys = ['kind', 'name', 'lon', 'lat'];
  pbf.writeMessage(3, (_, layer) => {
    layer.writeVarintField(15, 2);
    layer.writeStringField(1, 'pois');
    places.forEach((_, i) => layer.writeMessage(2, (__, feature) => {
      feature.writeVarintField(1, i + 1);
      feature.writePackedVarint(2, keys.flatMap((___, k) => [k, i * keys.length + k]));
      feature.writeVarintField(3, 1);
      feature.writePackedVarint(4, [9, 4096, 4096]);
    }));
    for (const key of keys) layer.writeStringField(3, key);
    for (const { name, lon, lat } of places) {
      for (const text of ['restaurant', name]) layer.writeMessage(4, (__, value) => value.writeStringField(1, text));
      for (const number of [lon, lat]) layer.writeMessage(4, (__, value) => value.writeDoubleField(3, number));
    }
    layer.writeVarintField(5, 4096);
  });
  return Buffer.from(pbf.finish());
}

// One restaurant near the climb, and 35 at one place near the second waypoint.
const PLACES = placeTile([{ name: 'Linde', lon: 8.03, lat: 47.001 },
  ...Array.from({ length: 35 }, (_, i) => ({ name: `Stube ${i + 1}`, lon: 8.15, lat: 47.001 + i * 1e-5 }))]);

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

test('the rows of a route of two days: a day, the pointer, a part, a climb, the keys, places and the state on the device', async ({ page }) => {
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.route(url => url.hostname !== '127.0.0.1', request => {
    if (request.request().method() === 'OPTIONS') return request.fulfill(CORS_PREFLIGHT);
    if (/^\/places\/\d+\/\d+\/\d+\.mvt$/.test(new URL(request.request().url()).pathname)) {
      return request.fulfill({ headers: CORS_PREFLIGHT.headers, contentType: 'application/x-protobuf', body: PLACES });
    }
    const response = respond(request.request());
    return response ? request.fulfill(response) : request.abort();
  });
  await page.goto('/planner.html#/route/two');
  await expect(page.getByText('This route is not in the library.')).toBeVisible();
  await expect(page.locator('.lanes')).toHaveCount(0);

  await page.goto('/planner.html?cover=none');
  await page.waitForFunction(() => window.coverTest);
  await page.evaluate(async record => {
    const { library, sentKey } = window.coverTest;
    const saved = await library.saveRoute(record);
    await library.changeState('sent', () => ({ two: { device: 'card:5', key: sentKey(saved) } }));
    localStorage.setItem('obc-device', JSON.stringify({ serial: 'A1B2', storeId: 'card', at: null, map: null, routes: 1, bytes: { map: 0, routes: 0, rides: 0, other: 0 }, came: null, skipped: [] }));
  }, route);
  await page.goto('/planner.html#/route/two');

  const lanes = page.locator('.lanes');
  for (const label of ['Waypoints', 'Elevation']) await expect(lanes.locator('.label').filter({ hasText: label })).toBeVisible();
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

  // A drag selects a part; Escape and Clear end it.
  const part = page.locator('.rows .part');
  const drag = async (from, to) => {
    const box = await lanes.locator('.lane').nth(1).boundingBox(), y = box.y + box.height / 2;
    await page.mouse.move(box.x + box.width * from, y);
    await page.mouse.down();
    await page.mouse.move(box.x + box.width * (from + to) / 2, y);
    await page.mouse.move(box.x + box.width * to, y);
    await page.mouse.up();
  };
  await drag(0.4, 0.6);
  await expect(part).toContainText('km 6.1 to km 9.1 · 3.0 km');
  await expect(lanes.locator('.pale')).toHaveCount(2);
  await page.keyboard.press('Escape');
  await expect(part).toHaveCount(0);
  await drag(0.4, 0.6);
  await part.getByRole('button', { name: 'Clear' }).click();
  await expect(part).toHaveCount(0);

  // A click on the climb in the profile and a click on it in the left card give the same part.
  await page.mouse.click(elevation.x + elevation.width * 0.1, elevation.y + elevation.height / 2);
  await expect(page.getByRole('button', { name: '‹ Climbs' })).toBeVisible();
  await expect(page.locator('.facts dl > div').filter({ hasText: 'Steepest' })).toContainText('5 %');
  const climb = await part.textContent();
  expect(climb).toMatch(/^km\s0.0 to km\s3.8 · 3.8\skm · 200\sm up/);
  // The map fits the climb: the pins beyond it leave the map.
  await expect(mapPins.locator('button')).toHaveText(['1']);
  await page.getByRole('button', { name: '‹ Climbs' }).click();
  await expect(part).toHaveCount(0);
  await page.locator('.facts .climb button').first().click();
  await expect(part).toHaveText(climb);

  // The keys: an arrow moves the pointer, Shift with an arrow grows a part, Enter selects the climb at the pointer.
  await page.keyboard.press('Escape');
  await page.getByRole('slider', { name: 'Profile' }).focus();
  await page.keyboard.press('ArrowRight');
  await expect(lanes.locator('.chip')).toContainText(/^15\d\sm/);
  for (let i = 0; i < 5; i++) await page.keyboard.press('Shift+ArrowRight');
  await expect(part).toContainText('km 0.2 to km 0.9');
  await page.keyboard.press('Escape');
  await page.keyboard.press('Enter');
  await expect(part).toHaveText(climb);

  // Another day clears the part.
  await page.getByRole('radio', { name: 'Day 2' }).click();
  await expect(part).toHaveCount(0);
  await expect(page.getByRole('button', { name: '‹ Climbs' })).toHaveCount(0);
  await page.getByRole('radio', { name: 'Both days' }).click();

  // A place opens its name and shows on the map; a group lists at most 30 names.
  await page.getByRole('button', { name: 'Places ▾' }).click();
  await page.getByRole('button', { name: 'Food', exact: true }).click();
  await lanes.locator('.places').scrollIntoViewIfNeeded();
  const places = lanes.locator('.place > button');
  await expect(places).toHaveCount(2);
  await places.first().click();
  await expect(lanes.locator('.place .panel')).toContainText('Linde');
  await expect(page.locator('.route-map-place')).toBeVisible();
  await places.last().click();
  await expect(places.last()).toContainText('35');
  await expect(lanes.locator('.list button')).toHaveCount(30);
  await expect(lanes.locator('.list')).toContainText('and 5 more');
  await lanes.locator('.list button').first().click();
  await expect(lanes.locator('.place .panel')).toContainText('Stube 1');

  // Enter on a place opens it and leaves the profile alone; Escape closes the panel before it clears a part.
  await drag(0.4, 0.6);
  await places.first().focus();
  await page.keyboard.press('Enter');
  await expect(lanes.locator('.place .panel')).toContainText('Linde');
  await expect(part).toContainText('km 6.1 to km 9.1');
  await page.keyboard.press('Escape');
  await expect(lanes.locator('.place .panel')).toHaveCount(0);
  await expect(part).toHaveCount(1);
  await page.keyboard.press('Escape');
  await expect(part).toHaveCount(0);

  await page.evaluate(() => new Promise(resolve => {
    const open = indexedDB.open('obc-library');
    open.onsuccess = () => {
      const transaction = open.result.transaction('state', 'readwrite');
      transaction.objectStore('state').put({ two: { device: 'card:5', key: 'another' } }, 'sent');
      transaction.oncomplete = resolve;
    };
  }));
  await page.reload();
  await expect(page.getByText('Changed since it was sent')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Send to device' })).toBeEnabled();
  expect(errors).toEqual([]);
});
