import { readFile } from 'node:fs/promises';
import { expect, test } from '@playwright/test';
import { CORS_PREFLIGHT, respond } from './stubs.js';

const drawn = (id, kind, lat, from, to) => ({
  id, kind, label: id === 'view' ? 'Viewpoint' : id, coordinate: [8, lat],
  ...kind !== 'start' && { leg: 'drawn', drawn: [[8, lat - 0.1, from], [8, lat - 0.05, (from + to) / 2], [8, lat, to]] },
});

// A route of one day with drawn legs, so its line needs no routing request: 22.2 km along a meridian, up 400 m and down.
const route = {
  id: 'lake', name: 'Lake test', updatedAt: 0,
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
  // A draft that the planner cannot open, so the planner below shows no reminder.
  await page.evaluate(() => window.coverTest.library.saveDraft({ id: 'lake', updatedAt: 0, trip: {} }));
  await page.goto('/planner.html#/route/lake');
  await expect(page.getByText('This route has unsaved changes in the planner.')).toBeVisible();
  const facts = page.locator('.facts');
  await expect(page.getByRole('heading', { name: 'Lake test', level: 1 })).toBeVisible();
  // The map shows the relief, so the credit of the terrain is behind the button of credits.
  await expect(page.locator('.credits small')).toContainText('Terrain test data');
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

  await page.getByRole('link', { name: 'Open in planner' }).click();
  await expect(page.getByRole('heading', { name: 'Lake loop', exact: true })).toBeVisible();
  await expect(page).toHaveURL(/#\/planner$/);
  await page.goBack();
  await expect(page.getByRole('heading', { name: 'Lake loop', level: 1 })).toBeVisible();

  // "Share" is the first item of "More". Its window gives the share card as a JPEG, 1920 pixels wide: the picture of
  // 1920 × 1280 and the ground of a name of one line.
  await page.getByRole('button', { name: 'More ▾' }).click();
  await expect(page.locator('menu button').first()).toHaveText('Share');
  await page.getByRole('button', { name: 'Share' }).click();
  const share = page.getByRole('dialog', { name: 'Share' });
  // This browser makes no cover, so the window makes the picture of the map: then the card has a picture to switch off.
  for (const name of ['Picture', 'Line', 'Numbers', 'Elevation profile', 'Hide start and end']) await expect(share.getByRole('switch', { name, exact: true })).toBeVisible();
  await expect(share.getByRole('switch')).toHaveCount(5);
  await expect(share.getByRole('switch', { name: 'Hide start and end' })).toHaveAccessibleDescription('The first and last 500 m');
  // The planner saved the summary again when it opened the plan, with its own time.
  await expect(share.getByRole('img', { name: /^Share card: Lake loop, 22\.2 km, 400 m up, 1h \d+m$/ })).toBeVisible();
  const [card] = await Promise.all([page.waitForEvent('download'), share.getByRole('button', { name: 'Download' }).click()]);
  expect(card.suggestedFilename()).toBe('Lake loop.jpg');
  const bytes = await readFile(await card.path());
  expect([...bytes.subarray(0, 3)]).toEqual([0xff, 0xd8, 0xff]);
  // The size of an image file, and the colour of a pixel of the ground and of a board of the signpost.
  const sample = async ({ file, ground, board }) => {
    const image = await createImageBitmap(new Blob([new Uint8Array(file)]));
    const context = new OffscreenCanvas(image.width, image.height).getContext('2d');
    context.drawImage(image, 0, 0);
    const pixel = ([x, y]) => [...context.getImageData(x, y, 1, 1).data.slice(0, 3)];
    return { size: [image.width, image.height], ground: pixel(ground), board: pixel(board) };
  };
  const drawn = await page.evaluate(sample, { file: [...bytes], ground: [1900, 1300], board: [111, 83] });
  expect(drawn.size).toEqual([1920, 1780]);
  // The card is drawn: the plain ground in its right margin, and a board of the signpost of the product mark.
  const near = (pixel, colour) => pixel.every((value, i) => Math.abs(value - colour[i]) <= 24);
  expect(near(drawn.ground, [0xf4, 0xf2, 0xeb]), `ground ${drawn.ground}`).toBe(true);
  expect(near(drawn.board, [0xf4, 0xa8, 0x1d]), `board ${drawn.board}`).toBe(true);
  // The square format: the same card in 1280 × 1280, with the picture whole at the top.
  await share.getByRole('radio', { name: 'Square' }).click();
  await expect(share.getByRole('radio', { name: 'Square' })).toBeChecked();
  const [square] = await Promise.all([page.waitForEvent('download'), share.getByRole('button', { name: 'Download' }).click()]);
  const squareDrawn = await page.evaluate(sample, { file: [...await readFile(await square.path())], ground: [1270, 870], board: [74, 55] });
  expect(squareDrawn.size).toEqual([1280, 1280]);
  expect(near(squareDrawn.ground, [0xf4, 0xf2, 0xeb]), `ground ${squareDrawn.ground}`).toBe(true);
  expect(near(squareDrawn.board, [0xf4, 0xa8, 0x1d]), `board ${squareDrawn.board}`).toBe(true);
  // "Copy" puts the card of the format on the clipboard as a PNG. The share sheet of a system has no test here.
  await page.context().grantPermissions(['clipboard-read', 'clipboard-write']);
  await share.getByRole('button', { name: 'Copy' }).click();
  await expect(share.getByRole('status')).toHaveText('Copied.');
  expect(await page.evaluate(async () => {
    const [item] = await navigator.clipboard.read();
    const image = await createImageBitmap(await item.getType('image/png'));
    return [image.width, image.height];
  })).toEqual([1280, 1280]);
  // A copy that the browser refuses says one sentence in the window.
  await page.evaluate(() => { navigator.clipboard.write = () => Promise.reject(new DOMException('refused', 'NotAllowedError')); });
  await share.getByRole('button', { name: 'Copy' }).click();
  await expect(share.getByRole('status')).toHaveText('The browser did not copy the image.');
  await page.keyboard.press('Escape');
  await expect(share).toHaveCount(0);

  await page.getByRole('button', { name: 'More ▾' }).click();
  await page.getByRole('button', { name: 'Duplicate' }).click();
  await expect(page.getByRole('heading', { name: 'Lake loop (copy)', level: 1 })).toBeVisible();
  await page.getByRole('button', { name: 'More ▾' }).click();
  await page.getByRole('button', { name: 'Delete' }).click();
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
    const route = await library.saveRoute({ id: 'covered', name: 'Covered', updatedAt: 0, trip: {}, summary });
    await library.saveCover({ id: 'covered', lineKey: lineKey(route), state: 'done', tries: 0, image: new Blob([new Uint8Array(image)], { type: 'image/webp' }),
      points: { x: Float32Array.from(points.x), y: Float32Array.from(points.y), eye: Uint8Array.from(points.eye) } });
  }, { image, summary, points });

  // This page has no bottom card, so its picture is tall: the cover is lower than the picture and leaves a third to the map.
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto('/planner.html#/route/covered');
  const cover = page.locator('.slot.cover'), map = page.locator('.slot.map');
  await expect(cover.locator('canvas')).toBeVisible();
  const [coverBox, mapBox, pictureBox] = [await cover.boundingBox(), await map.boundingBox(), await page.locator('.picture').boundingBox()];
  // The shape of the stored covers, 1920 × 1280.
  expect(coverBox.width / coverBox.height).toBeCloseTo(1.5, 1);
  expect(mapBox.x).toBeGreaterThanOrEqual(coverBox.x + coverBox.width + 13);
  expect(mapBox.width).toBeCloseTo(pictureBox.width / 3, -1);
  expect(mapBox.height).toBeCloseTo(coverBox.height, 0);
  expect(coverBox.height).toBeLessThan(pictureBox.height - 100);
  await expect(page.getByRole('button', { name: 'Show the map large' })).toHaveCount(0);

  await page.setViewportSize({ width: 900, height: 900 });
  await expect(page.getByRole('button', { name: 'Show the map large' })).toBeVisible();
});
