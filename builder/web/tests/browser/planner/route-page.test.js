import { readFile } from 'node:fs/promises';
import { expect, test } from '@playwright/test';
import { CORS_PREFLIGHT, respond } from './stubs.js';

const drawn = (id, kind, lat, from, to) => ({
  id, kind, label: id === 'view' ? 'Viewpoint' : id, coordinate: [8, lat],
  ...kind !== 'start' && { leg: 'drawn', drawn: [[8, lat - 0.1, from], [8, lat - 0.05, (from + to) / 2], [8, lat, to]] },
});

// A route of one day with drawn legs, so its line needs no routing request: 22.2 km along a meridian, up 400 m and down.
const route = {
  id: 'lake', revision: 0, name: 'Lake test', updatedAt: 0,
  trip: { mode: 'route', bike: 'touring', days: 1, routeOrder: ['view'], points: [drawn('start', 'start', 48), drawn('view', 'waypoint', 48.1, 300, 700), drawn('finish', 'finish', 48.2, 700, 300)] },
  summary: { line: [8, 48, 8, 48.1, 8, 48.2], heights: [300, 700, 300], bounds: [8, 48, 8, 48.2], lengthM: 22_239, climbM: 400, highestM: 700, seconds: 4000, bike: 'touring', days: 1 },
};

test('the route page shows a stored route, renames, sends, opens in the planner, duplicates and deletes', async ({ page }) => {
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.route(url => url.hostname !== '127.0.0.1', request => {
    if (request.request().method() === 'OPTIONS') return request.fulfill(CORS_PREFLIGHT);
    const response = respond(request.request());
    return response ? request.fulfill(response) : request.abort();
  });
  await page.goto('/planner.html#/route/lake');
  await expect(page.getByText('This route is not in the library.')).toBeVisible();

  await page.goto('/planner.html?cover=none');
  await page.waitForFunction(() => window.coverTest);
  await page.evaluate(record => window.coverTest.library.saveRoute(record), route);
  await page.goto('/planner.html#/route/lake');
  const facts = page.locator('.facts');
  await expect(page.getByRole('heading', { name: 'Lake test', level: 1 })).toBeVisible();
  for (const [label, value] of [['Length', '22.2 km'], ['Climb', '400 m'], ['Time', '1h 7m'], ['Highest point', '700 m']]) {
    await expect(facts.locator('dl > div').filter({ hasText: label })).toContainText(value);
  }
  await expect(facts).toContainText('km 0 · 11.1 km · 4 %');
  await expect(facts.locator('.stop')).toContainText('Viewpoint');
  await expect(facts.locator('.stop')).toContainText('km 11');
  await expect(facts).not.toContainText('Changed since the last save');

  await page.getByRole('button', { name: 'Rename' }).click();
  await page.getByRole('textbox', { name: 'Route name' }).fill('Lake loop');
  await page.keyboard.press('Enter');
  await expect(page.getByRole('heading', { name: 'Lake loop', level: 1 })).toBeVisible();

  await page.getByRole('button', { name: 'Send to device' }).click();
  await expect(page.getByText('Waits for the device')).toBeVisible();
  await page.getByRole('button', { name: 'Do not send' }).click();
  await expect(page.getByText('Waits for the device')).toHaveCount(0);

  await page.getByRole('link', { name: 'Open in the planner' }).click();
  await expect(page.getByRole('heading', { name: 'Lake loop', exact: true })).toBeVisible();
  await expect(page).toHaveURL(/#\/planner$/);
  await page.goBack();
  await expect(page.getByRole('heading', { name: 'Lake loop', level: 1 })).toBeVisible();

  await page.getByRole('button', { name: 'Duplicate' }).click();
  await expect(page.getByRole('heading', { name: 'Lake loop (copy)', level: 1 })).toBeVisible();
  await page.locator('.facts').getByRole('button', { name: 'Delete' }).click();
  await page.getByRole('dialog', { name: 'Delete this route?' }).getByRole('button', { name: 'Delete' }).click();
  await expect(page).toHaveURL(/#\/routes$/);
  await page.evaluate(() => { location.hash = '#/route/lake'; });
  await expect(page.getByRole('heading', { name: 'Lake loop', level: 1 })).toBeVisible();
  expect(errors).toEqual([]);
});

test('the cover shows whole with the map beside it, and with the map in its corner when the window is narrow', async ({ page }) => {
  await page.route(url => url.hostname !== '127.0.0.1', request => {
    if (request.request().method() === 'OPTIONS') return request.fulfill(CORS_PREFLIGHT);
    const response = respond(request.request());
    return response ? request.fulfill(response) : request.abort();
  });
  // A real cover of a route at a quarter of its size, with its points.
  const testdata = new URL('../../../src/lib/covers/testdata/', import.meta.url);
  const image = [...await readFile(new URL('cover.webp', testdata))];
  const { summary, points } = JSON.parse(await readFile(new URL('cover.json', testdata), 'utf8'));
  await page.goto('/planner.html?cover=none');
  await page.waitForFunction(() => window.coverTest);
  await page.evaluate(async ({ image, summary, points }) => {
    const { library, lineKey } = window.coverTest;
    const route = await library.saveRoute({ id: 'covered', revision: 0, name: 'Covered', updatedAt: 0, trip: {}, summary });
    await library.saveCover({ id: 'covered', lineKey: lineKey(route), state: 'done', tries: 0, image: new Blob([new Uint8Array(image)], { type: 'image/webp' }),
      points: { x: Float32Array.from(points.x), y: Float32Array.from(points.y), eye: Uint8Array.from(points.eye) } });
  }, { image, summary, points });

  // This page has no bottom card, so its picture is tall.
  await page.setViewportSize({ width: 1920, height: 900 });
  await page.goto('/planner.html#/route/covered');
  const cover = page.locator('.slot.cover'), map = page.locator('.slot.map');
  await expect(cover.locator('canvas')).toBeVisible();
  const [coverBox, mapBox] = [await cover.boundingBox(), await map.boundingBox()];
  // The shape of the stored covers, 1920 × 1280.
  expect(coverBox.width / coverBox.height).toBeCloseTo(1.5, 1);
  expect(mapBox.x).toBeGreaterThanOrEqual(coverBox.x + coverBox.width + 13);
  expect(mapBox.width).toBeGreaterThanOrEqual(260);
  expect(mapBox.height).toBeCloseTo(coverBox.height, 0);
  await expect(page.getByRole('button', { name: 'Show the map large' })).toHaveCount(0);

  await page.setViewportSize({ width: 900, height: 900 });
  await expect(page.getByRole('button', { name: 'Show the map large' })).toBeVisible();
});
