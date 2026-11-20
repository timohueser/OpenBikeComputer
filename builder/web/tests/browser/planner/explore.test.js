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

test('the Explore page searches the day hikes of the map view, plans one and comes back to the same search', async ({ page }) => {
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
  const results = page.locator('.results'), rows = results.locator('tr.row');
  await expect(results.getByText('Zoom in to search.')).toBeVisible();
  await expect(page.getByRole('link', { name: 'Explore' })).toHaveAttribute('aria-current', 'page');

  await page.getByRole('searchbox', { name: 'Find a place' }).fill('Freiburg');
  await page.getByRole('button', { name: /^Freiburg/ }).click();
  // The least paved first; the long route is no row, the bike routes are not for hiking, and the stage is there and back.
  await expect(results.locator('.count')).toHaveText('2 hikes');
  await expect(rows).toHaveText([/Schlossberg loop\s*Loop\s*1\.2\skm\s*150\sm\s*–\s*T2\s*PLarge car park/, /Westweg 2\s*There and back\s*5\.0\skm\s*100\sm\s*2h 50m\s*T1\s*No car park/]);
  await page.getByRole('button', { name: 'TrailView' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'TrailView' })).toHaveText('TrailView has no view of this area.');

  // A must wish keeps only the hikes that meet it, and the line over the table names it.
  const summit = page.locator('.wish', { hasText: 'Summit' });
  await expect(summit.locator('.count')).toHaveText('1');
  await summit.getByRole('button', { name: 'Summit' }).click();
  await summit.getByRole('button', { name: 'Must' }).click();
  await expect(rows).toHaveText([/Schlossberg loop/]);
  await expect(results.locator('.query')).toHaveText('In the map view · must: summit.');
  // The page address holds the query and the view, and a new load of the page reads it.
  await expect(page).toHaveURL(/#\/explore\?must=peak&view=[\d.,]+$/);
  await page.reload();
  await expect(rows).toHaveText([/Schlossberg loop/]);
  await expect(summit.getByRole('button', { name: 'Must' })).toHaveAttribute('aria-pressed', 'true');

  await summit.getByRole('button', { name: 'Summit' }).click();
  await page.getByRole('button', { name: 'Loop', exact: true }).click();
  await expect(rows).toHaveText([/Westweg 2/]);
  // A row opens in place, with the link to the page of its route.
  await rows.getByRole('button', { name: /Westweg 2/ }).click();
  const open = results.locator('tr.more');
  await expect(open).toContainText('There and back, 2.5 km each way');
  await expect(open.getByRole('link', { name: 'Westweg 2' })).toHaveAttribute('href', '#/signed/9-267-177/202');
  const kept = await page.evaluate(() => new Promise(done => setTimeout(() => done(history.state.explore), 500)));
  expect(kept.query).toBe('shapes=back,liftDown,liftUp');

  await open.getByRole('button', { name: 'Plan this route' }).click();
  await expect(page).toHaveURL(/#\/planner$/);
  await expect(page.getByText(/Westweg 2 planned/)).toBeVisible();
  // The planned route is not stored until the rider saves it.
  await expect(page.locator('.trip-bar .save > button')).toHaveText('Save');
  expect(await page.evaluate(() => new Promise(resolve => {
    const request = indexedDB.open('obc-library');
    request.onsuccess = () => { const all = request.result.transaction('routes').objectStore('routes').count(); all.onsuccess = () => { request.result.close(); resolve(all.result); }; };
  }))).toBe(0);
  await page.getByRole('button', { name: 'Find another route' }).click();

  await expect(page).toHaveURL(/#\/explore\?shapes=back,liftDown,liftUp&view=/);
  await expect(results.locator('tr.row.open')).toContainText('Westweg 2');
  // Until the map loads, the page keeps the view of the history; after it, the view is the map's own.
  await expect(page.locator('.routes-map')).toHaveAttribute('data-loaded', 'true');
  const back = await page.evaluate(() => new Promise(done => setTimeout(() => done(history.state.explore), 500)));
  expect([back.chips, back.query, back.selected, back.shape]).toEqual([kept.chips, kept.query, '202', 'back']);
  for (const [i, value] of kept.view.entries()) expect(back.view[i]).toBeCloseTo(value, 4);

  // A bike keeps the list with chips, and the plain address.
  await page.locator('.chip summary', { hasText: 'Hiking' }).click();
  await page.getByRole('button', { name: 'Mountain bike' }).click();
  await expect(results).toHaveCount(0);
  await expect(page.locator('.list .row')).toHaveText([/Schlossberg trail.*1\.2\skm · 150\sm up · S1/, /Rosskopf trail.*2\.5\skm · 80\sm up · S0/]);
  await expect(page).toHaveURL(/#\/explore$/);
  expect(errors).toEqual([]);
});

test('the name of an open row opens the page of the route, and Back and "‹ Explore" open the row again', async ({ page }) => {
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  const open = await openRow(page, 'Westweg 2', 1440, 900);
  // A nice wish is the last search of the browser.
  await page.locator('.wish', { hasText: 'Summit' }).getByRole('button', { name: 'Summit' }).click();
  let answer;
  const held = new Promise(done => answer = done);
  await page.route(url => url.pathname.endsWith('/v1/route'), async route => { await held; await route.fallback(); });
  await open.getByRole('link', { name: 'Westweg 2' }).click();
  await expect(page).toHaveURL(/#\/signed\/9-267-177\/202$/);
  await expect(page.getByRole('heading', { name: 'Westweg 2' })).toBeVisible();
  // The Time is the stored time of the record, before and after the routing answer, whose own time is a few minutes.
  const time = page.locator('.numbers div', { hasText: 'Time' }).locator('dd');
  await expect(time).toHaveText('1h 30m');
  await expect(page.getByText('Finding the climbs…')).toBeVisible();
  answer();
  await expect(page.getByText('Finding the climbs…')).toHaveCount(0);
  await expect(time).toHaveText('1h 30m');
  await expect(page.locator('.title')).toContainText('Stage 2 of 2 · Westweg');
  await expect(page.getByRole('button', { name: 'Plan this route' })).toBeVisible();
  await expect(page.getByRole('link', { name: 'Explore', exact: true })).toHaveAttribute('aria-current', 'page');

  await page.goBack();
  await expect(open.getByRole('link', { name: 'Westweg 2' })).toBeVisible();
  await open.getByRole('link', { name: 'Westweg 2' }).click();
  await expect(page).toHaveURL(/#\/signed\/9-267-177\/202$/);
  // Coming from the row, "‹ Explore" is Back.
  await page.getByRole('link', { name: '‹ Explore' }).click();
  await expect(open.getByRole('link', { name: 'Westweg 2' })).toBeVisible();
  await page.goForward();
  await expect(page).toHaveURL(/#\/signed\/9-267-177\/202$/);

  // After a reload, "‹ Explore" opens the search of the browser on the route; the test build has no far files, so it is a day route.
  for (const [name, address] of [['Westweg 2', '9-267-177/202'], ['Schlossberg loop', '9-267-177/101']]) {
    await page.goto(`/planner.html#/signed/${address}`);
    await page.reload();
    await expect(page.getByRole('heading', { name })).toBeVisible();
    await page.getByRole('link', { name: '‹ Explore' }).click();
    await expect(page).toHaveURL(/#\/explore(\?.*)?$/);
    await expect(page.locator('.results tr.row.open')).toContainText(name);
    await expect(page.locator('.wish', { hasText: 'Summit' }).getByRole('button', { name: 'Summit' })).toHaveAttribute('aria-pressed', 'true');
  }

  // A last search that leaves the route out gives way to the default search, so the row of the route opens.
  await page.locator('.wish', { hasText: 'Summit' }).getByRole('button', { name: 'Must' }).click();
  await expect(page).toHaveURL(/must=peak/);
  await page.goto('/planner.html#/signed/9-267-177/202');
  await page.reload();
  await page.getByRole('link', { name: '‹ Explore' }).click();
  await expect(page.locator('.results tr.row.open')).toContainText('Westweg 2');
  await expect(page.locator('.wish', { hasText: 'Summit' }).getByRole('button', { name: 'Summit' })).toHaveAttribute('aria-pressed', 'false');

  await page.goto('/planner.html#/signed/9-267-177/999');
  await expect(page.getByText('This route is no longer in the map data.')).toBeVisible();
  expect(errors).toEqual([]);
});

test('with a start point, a view of more than 12 cells searches the cells in reach, with the drive of its cap', async ({ page }) => {
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await stub(page);
  // The whole area of the test build. At Freiburg the drive is at most 40 km; the stage is 2 km away and has no car park.
  await page.goto('/planner.html#/explore?start=7.85,47.99&car=1&drive=45&view=7.45,47.5,10.5,49.85');
  const results = page.locator('.results');
  await expect(results.locator('.query')).toHaveText('From the start point: walk 1 km, drive 40 km.');
  await expect(results.locator('tr.row')).toHaveText([/Schlossberg loop/]);
  expect(errors).toEqual([]);
});

test('the trail name on a stage page opens the page of the trail, whose rows open the pages of its stages', async ({ page }) => {
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await stub(page);
  await page.goto('/planner.html#/signed/9-267-177/202');
  await page.locator('.title').getByRole('link', { name: 'Westweg', exact: true }).click();
  await expect(page).toHaveURL(/#\/trail\/300$/);
  await expect(page.getByRole('heading', { name: 'Westweg', exact: true })).toBeVisible();
  await expect(page.locator('.title')).toContainText('1 of 3 stages');
  // Stage 1 is a gap; stage 3 is outside the release and has no row.
  const rows = page.locator('.facts li');
  await expect(rows).toHaveText([/^Stage 1.*Not in the map data/, /^Stage 2.*1h 30m/]);
  await expect(rows.nth(0)).toHaveClass(/\bgap\b/);
  await expect(rows.nth(0).getByRole('link')).toHaveCount(0);

  await rows.nth(1).getByRole('link').click();
  await expect(page).toHaveURL(/#\/signed\/9-267-177\/202$/);
  await page.goBack();
  await expect(page).toHaveURL(/#\/trail\/300$/);
  await expect(rows).toHaveCount(2);
  // After a reload no page is before this one: "‹ Explore" opens Explore on the trail, in the box of its stage lines. The
  // click writes that state at once; Explore writes its own state later.
  await page.reload();
  await expect(page.locator('.routes-map')).toBeVisible();
  const state = await page.evaluate(() => {
    [...document.querySelectorAll('a')].find(link => link.textContent === '‹ Explore').click();
    return history.state.explore;
  });
  expect(state).toEqual({ chips: { bike: 'hiking' }, view: [7.88, 47.98, 7.91, 48].map(value => expect.closeTo(value, 6)), tab: 'long', trail: 300 });
  await expect(page).toHaveURL(/#\/explore$/);

  await page.goto('/planner.html#/trail/1');
  await expect(page.getByText('This trail is no longer in the map data.')).toBeVisible();
  expect(errors).toEqual([]);
});

/** Opens the row of the day hike `name` in a window of `width` × `height` px, and gives its open part. */
async function openRow(page, name, width, height) {
  await findFreiburg(page, width, height);
  await page.locator('.results').getByRole('button', { name: new RegExp(name) }).click();
  return page.locator('.results tr.more');
}

/** Shows the card of the mountain bike route `name` in a window of `width` × `height` px: a bike keeps the list and the card. */
async function openCard(page, name, width, height) {
  await findFreiburg(page, width, height);
  await page.locator('.chip summary', { hasText: 'Hiking' }).click();
  await page.getByRole('button', { name: 'Mountain bike' }).click();
  await page.locator('.list').getByRole('button', { name: new RegExp(name) }).click();
  return page.getByRole('region', { name });
}

async function findFreiburg(page, width, height) {
  await page.setViewportSize({ width, height });
  await stub(page);
  await page.goto('/planner.html#/explore');
  await page.getByRole('searchbox', { name: 'Find a place' }).fill('Freiburg');
  await page.getByRole('button', { name: /^Freiburg/ }).click();
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
  const card = await openCard(page, 'Rosskopf trail', 390, 844);
  // The card is never higher than the screen less 96 px.
  await expect.poll(() => card.evaluate(node => [!node.querySelector('.profile'), node.getBoundingClientRect().height <= innerHeight - 96]))
    .toEqual([true, true]);
  await textScrolls(page, card, 1);
});

// 844 px wide has the layout of a computer, and 667 px wide that of a phone.
for (const [width, height] of [[844, 390], [667, 375]]) {
  test(`in a window of ${width} × ${height} px, the card of a route with a profile shows no strip, and two lines of its text that scroll`, async ({ page }) => {
    const card = await openCard(page, 'Schlossberg trail', width, height);
    await expect(card.locator('.profile')).toBeHidden();
    await textScrolls(page, card, 2);
  });
}

test('in a wide and low window, the card is in the map with its close button, and a lower picture leaves three lines of its text', async ({ page }) => {
  const card = await openCard(page, 'Rosskopf trail', 1440, 420);
  await textScrolls(page, card, 3);
  const close = card.getByRole('button', { name: 'Close' });
  await expect.poll(async () => {
    const [map, button] = await Promise.all([page.locator('.map').boundingBox(), close.boundingBox()]);
    return button.y >= map.y && button.y + button.height <= map.y + map.height;
  }).toBe(true);
  await close.click();
  await expect(card).toHaveCount(0);
});
