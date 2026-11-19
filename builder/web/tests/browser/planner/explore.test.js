import { expect, test } from '@playwright/test';
import { CORS_PREFLIGHT, respond } from './stubs.js';

// A town around the routes of the catalog in `stubs.js`.
const FREIBURG = { name: 'Freiburg', kind: 'city', lon: 7.85, lat: 47.99, west: 7.8, south: 47.96, east: 7.92, north: 48.02, region: 'Testland' };

function stub(page) {
  return page.route(url => url.hostname !== '127.0.0.1', route => {
    const request = route.request();
    if (request.method() === 'OPTIONS') return route.fulfill(CORS_PREFLIGHT);
    if (request.url().endsWith('/query')) return route.fulfill({ ...CORS_PREFLIGHT, status: 200, json: { type: 'places', request: { type: 'place' }, results: [FREIBURG] } });
    // No terrain tile: the cover view has no heights, so it finds no camera for the routes.
    if (new URL(request.url()).pathname.startsWith('/terrain/')) return route.fulfill({ ...CORS_PREFLIGHT, status: 204 });
    const response = respond(request);
    return response ? route.fulfill(response) : route.abort();
  });
}

test('the Explore page lists the signed routes of the map view, filters them, plans one and comes back to the same view', async ({ page }) => {
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  // A GPU without an adapter, so the page offers TrailView in a headless browser.
  await page.addInitScript(() => Object.defineProperty(Navigator.prototype, 'gpu', {
    configurable: true,
    get: () => ({ requestAdapter: async () => null, getPreferredCanvasFormat: () => 'bgra8unorm' }),
  }));
  await stub(page);

  // The whole area of the test build needs more catalog files than one view may load.
  await page.goto('/planner.html#/explore');
  const list = page.locator('.list');
  await expect(list.getByText('Zoom in to see the signed routes.')).toBeVisible();
  await expect(page.getByRole('link', { name: 'Explore' })).toHaveAttribute('aria-current', 'page');

  await page.getByRole('searchbox', { name: 'Find a place' }).fill('Freiburg');
  await page.getByRole('button', { name: /^Freiburg/ }).click();
  // The shortest first; the long route is no row, and the cycle route is not for hiking.
  await expect(list.getByText('2 signed routes in this map view')).toBeVisible();
  await expect(list.locator('.row')).toHaveText([/Schlossberg loop.*1\.2\skm · 150\sm up · T2/, /Westweg 2.*2\.5\skm · 80\sm up · T1/]);
  // The set of route pictures holds a cover of the first route only; the other row shows its line on a plain ground.
  await expect(list.locator('.row').nth(0).locator('canvas')).toBeVisible();
  await expect(list.locator('.row').nth(1).locator('svg')).toBeVisible();
  await page.getByRole('button', { name: 'TrailView' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'TrailView' })).toHaveText('TrailView has no view of this area.');

  await page.getByText('+ Shape').click();
  await page.getByRole('button', { name: 'One way' }).click();
  await expect(list.getByText('1 of 2 signed routes')).toBeVisible();
  await list.getByRole('button', { name: /Westweg 2/ }).click();
  const card = page.getByRole('region', { name: 'Westweg 2' });
  await expect(card).toContainText('One way · Stage 2 of 2 · Westweg · National hiking route');
  // The button of the card closes it and clears the selection.
  await card.getByRole('button', { name: 'Close' }).click();
  await expect(card).toHaveCount(0);
  await expect(list.locator('.row[aria-current="true"]')).toHaveCount(0);
  await list.getByRole('button', { name: /Westweg 2/ }).click();
  const kept = await page.evaluate(() => new Promise(done => setTimeout(() => done(history.state.explore), 500)));

  await card.getByRole('button', { name: 'Plan this route' }).click();
  await expect(page).toHaveURL(/#\/planner$/);
  await expect(page.getByText(/Westweg 2 planned/)).toBeVisible();
  // The planned route is not stored until the rider saves it.
  await expect(page.locator('.trip-bar .save > button')).toHaveText('Save');
  expect(await page.evaluate(() => new Promise(resolve => {
    const request = indexedDB.open('obc-library');
    request.onsuccess = () => { const all = request.result.transaction('routes').objectStore('routes').count(); all.onsuccess = () => { request.result.close(); resolve(all.result); }; };
  }))).toBe(0);
  await page.getByRole('button', { name: 'Find another route' }).click();

  await expect(page).toHaveURL(/#\/explore$/);
  await expect(page.locator('.chip.set summary')).toHaveText(['Hiking ▾', 'One way ▾']);
  await expect(list.locator('.row[aria-current="true"]')).toContainText('Westweg 2');
  await expect(page.getByRole('region', { name: 'Westweg 2' })).toBeVisible();
  // Until the map loads, the page keeps the view of the history; after it, the view is the map's own.
  await expect(page.locator('.routes-map')).toHaveAttribute('data-loaded', 'true');
  const back = await page.evaluate(() => new Promise(done => setTimeout(() => done(history.state.explore), 500)));
  expect(back.chips).toEqual(kept.chips);
  for (const [i, value] of kept.view.entries()) expect(back.view[i]).toBeCloseTo(value, 4);
  expect(errors).toEqual([]);
});

/** Shows the card of the signed route `name` in a window of `width` × `height` px. */
async function openCard(page, name, width, height) {
  await page.setViewportSize({ width, height });
  await stub(page);
  await page.goto('/planner.html#/explore');
  await page.getByRole('searchbox', { name: 'Find a place' }).fill('Freiburg');
  await page.getByRole('button', { name: /^Freiburg/ }).click();
  await page.locator('.list').getByRole('button', { name: new RegExp(name) }).click();
  return page.getByRole('region', { name });
}

/** The text of the card shows at least `lines` lines on the screen, and the wheel over it scrolls it. */
async function textScrolls(page, card, lines) {
  const more = card.locator('.more');
  await expect.poll(() => more.evaluate((node, lines) => {
    const box = node.getBoundingClientRect(), line = parseFloat(getComputedStyle(node.firstElementChild).lineHeight);
    return Math.min(box.bottom, innerHeight) - Math.max(box.top, 0) >= lines * line;
  }, lines)).toBe(true);
  const box = await more.boundingBox();
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.wheel(0, 100);
  await expect.poll(() => more.evaluate(node => node.scrollTop)).toBeGreaterThan(0);
}

test('on a phone, the card of a route with a long text and no profile stays on the screen, and its text scrolls', async ({ page }) => {
  const card = await openCard(page, 'Westweg 2', 390, 844);
  // The card is never higher than the screen less 96 px.
  await expect.poll(() => card.evaluate(node => [!node.querySelector('.profile'), node.getBoundingClientRect().height <= innerHeight - 96]))
    .toEqual([true, true]);
  await textScrolls(page, card, 1);
});

// 844 px wide has the layout of a computer, and 667 px wide that of a phone.
for (const [width, height] of [[844, 390], [667, 375]]) {
  test(`in a window of ${width} × ${height} px, the card of a route with a profile shows no strip, and two lines of its text that scroll`, async ({ page }) => {
    const card = await openCard(page, 'Schlossberg loop', width, height);
    await expect(card.locator('.profile')).toBeHidden();
    await textScrolls(page, card, 2);
  });
}

test('in a wide and low window, the card is in the map with its close button, and a lower picture leaves three lines of its text', async ({ page }) => {
  const card = await openCard(page, 'Westweg 2', 1440, 420);
  await textScrolls(page, card, 3);
  const close = card.getByRole('button', { name: 'Close' });
  await expect.poll(async () => {
    const [map, button] = await Promise.all([page.locator('.map').boundingBox(), close.boundingBox()]);
    return button.y >= map.y && button.y + button.height <= map.y + map.height;
  }).toBe(true);
  await close.click();
  await expect(card).toHaveCount(0);
});
