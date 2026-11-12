import { readFile } from 'node:fs/promises';
import { expect, test } from '@playwright/test';

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
  // The mark goes to the point of the line nearest to the pointer, on both covers.
  await large.hover({ position: { x: 480, y: 240 } });
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
