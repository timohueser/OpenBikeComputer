import { readFile } from 'node:fs/promises';
import { expect, test } from '@playwright/test';
import { CORS_PREFLIGHT, classify, respond } from './stubs.js';

// A real cover of a route in central Switzerland, at a quarter of its size, with its points.
const testdata = new URL('../../../src/lib/covers/testdata/', import.meta.url);

/** The page of the cover of `id`, with the route `covered` and its cover, and the route `plain` with no cover. */
async function open(page, id) {
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  const image = [...await readFile(new URL('cover.webp', testdata))];
  const { summary, points } = JSON.parse(await readFile(new URL('cover.json', testdata), 'utf8'));
  await page.goto(`/planner.html?cover=${id}`);
  await page.waitForFunction(() => window.coverTest);
  // Points 120 to 159 are hidden as well, so the image shows a long weak stretch.
  const eye = points.eye.map((code, i) => i >= 120 && i < 160 ? code & 127 : code);
  await page.evaluate(async ({ image, summary, points, eye }) => {
    const { library, lineKey } = window.coverTest;
    const route = id => ({ id, revision: 0, name: id, updatedAt: 0, trip: {}, summary });
    for (const id of ['covered', 'plain']) await library.saveRoute(route(id));
    await library.saveCover({
      id: 'covered', lineKey: lineKey(route('covered')), state: 'done', tries: 0,
      image: new Blob([new Uint8Array(image)], { type: 'image/webp' }),
      points: { x: Float32Array.from(points.x), y: Float32Array.from(points.y), eye: Uint8Array.from(eye) },
    });
  }, { image, summary, points, eye });
  return { errors, summary };
}

test('a cover shows its line at a card size and at a large size, with the mark at the pointer', async ({ page }) => {
  const { errors } = await open(page, 'covered');
  const large = page.locator('.large canvas');
  await expect(page.locator('canvas')).toHaveCount(2);
  // The mark goes to the point of the line nearest to the pointer, on both covers; the pointer is near the line.
  await large.hover({ position: { x: 432, y: 345 } });
  await expect(page.locator('main')).toHaveScreenshot('cover.png');
  expect(errors).toEqual([]);
});

test('a route with no cover shows its line on a plain ground', async ({ page }) => {
  const { errors, summary } = await open(page, 'plain');
  await expect(page.locator('svg polyline')).toHaveCount(2);
  for (const line of await page.locator('svg polyline').all()) expect((await line.getAttribute('points')).split(' ')).toHaveLength(summary.line.length / 2);
  await expect(page.locator('main')).toHaveScreenshot('fallback.png');
  expect(errors).toEqual([]);
});

test('a card with no cover shows the map picture of the theme, and the large box the plain line', async ({ page }) => {
  const errors = [], requests = [];
  page.on('pageerror', error => errors.push(error.message));
  // No WebGPU, so the job makes no cover.
  await page.addInitScript(() => delete Navigator.prototype.gpu);
  await page.route(url => url.hostname !== '127.0.0.1', request => {
    if (request.request().method() === 'OPTIONS') return request.fulfill(CORS_PREFLIGHT);
    const url = new URL(request.request().url());
    requests.push(url.pathname.endsWith('.json') ? 'tilejson' : classify(url));
    const response = respond(request.request());
    return response ? request.fulfill(response) : request.abort();
  });
  // A loop of 24 points near Freiburg, inside the bounds of the test release.
  const ring = Array.from({ length: 24 }, (_, i) => [8.1 + 0.05 * Math.cos(i * Math.PI / 12), 48 + 0.035 * Math.sin(i * Math.PI / 12)]);
  const summary = {
    line: ring.flatMap(point => point.map(value => Math.round(value * 1e5) / 1e5)), heights: ring.map(() => null),
    bounds: [8.05, 47.965, 8.15, 48.035], lengthM: 22_000, climbM: 0, highestM: null, seconds: 4000, bike: 'road', days: 1,
  };
  await page.goto('/planner.html?cover=loop');
  await page.waitForFunction(() => window.coverTest);
  await page.evaluate(summary => window.coverTest.library.saveRoute({ id: 'loop', revision: 0, name: 'Loop', updatedAt: 0, trip: {}, summary }), summary);
  // The card asks when it loads a record with a line.
  await page.reload();
  await page.waitForFunction(() => window.coverTest);
  await page.evaluate(() => window.coverTest.startCoverJob());
  const map = () => page.evaluate(async () => {
    const picture = (await window.coverTest.library.cover('loop'))?.map;
    return { light: picture?.light?.size ?? 0, dark: picture?.dark?.size ?? 0 };
  });
  await expect.poll(async () => (await map()).light, { timeout: 30_000 }).toBeGreaterThan(0);
  await expect(page.locator('.card canvas')).toHaveCount(1);
  await expect(page.locator('.large svg polyline')).toHaveCount(1);
  const count = kind => requests.filter(request => request === kind).length;
  expect(count('tilejson')).toBe(1);
  expect(count('basemap')).toBeLessThanOrEqual(6);
  expect(count('terrain')).toBeLessThanOrEqual(6);
  expect(count('glyphs') + count('sprites')).toBe(0);

  await page.evaluate(() => window.coverTest.setLook('dark'));
  await expect.poll(async () => (await map()).dark, { timeout: 30_000 }).toBeGreaterThan(0);
  await expect(page.locator('.card canvas')).toHaveCount(1);
  expect(errors).toEqual([]);
});
