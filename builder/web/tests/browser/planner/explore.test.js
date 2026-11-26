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
  const list = listOf(page), rows = rowsOf(page);
  await expect(list.getByText('Zoom in to search.')).toBeVisible();
  await expect(page.getByRole('link', { name: 'Explore' })).toHaveAttribute('aria-current', 'page');

  await page.getByRole('combobox', { name: 'Say what you want' }).fill('Freiburg');
  await page.getByRole('option', { name: /^Freiburg/ }).click();
  // The least paved first; the long route is no row, the bike routes are not for hiking, and the stage is there and back.
  await expect(list.getByRole('heading')).toHaveText('2 hikes');
  await expect(rows).toHaveText([/^\s*Schlossberg loop\s*Loop · 1\.2\skm · 150\sm up · T2\s*PLarge car park\s*$/,
    /^\s*Westweg 2\s*There and back · 5\.0\skm · 100\sm up · 2h 50m · T1\s*No car park\s*$/]);
  // "Sort by" orders the rows and marks its order.
  await list.getByRole('button', { name: 'Sort by: Best match' }).click();
  await page.getByRole('button', { name: 'Climb' }).click();
  await expect(rows).toHaveText([/^\s*Westweg 2/, /^\s*Schlossberg loop/]);
  await list.getByRole('button', { name: 'Sort by: Climb' }).click();
  await expect(page.locator('menu button', { hasText: '✓' })).toHaveText('✓Climb');
  await page.getByRole('button', { name: 'Best match' }).click();
  await page.getByRole('button', { name: 'TrailView' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'TrailView' })).toHaveText('TrailView has no view of this area.');

  // A tick on a wish is a must: it keeps only the hikes that meet it, and its slot names it.
  await openSlot(page, 'way');
  const summit = page.locator('.wish', { hasText: 'Summit' });
  await expect(summit.locator('.count')).toHaveText('1');
  await summit.getByRole('button', { name: 'Summit' }).click();
  await expect(summit.getByRole('button', { name: 'Must' })).toHaveAttribute('aria-pressed', 'true');
  await expect(rows).toHaveText([/Schlossberg loop/]);
  await expect(slotText(page, 'way')).toHaveText('Summit');
  // The page address holds the query and the view, and a new load of the page reads it.
  await expect(page).toHaveURL(/#\/explore\?must=peak&view=[\d.,]+$/);
  await page.reload();
  await expect(rows).toHaveText([/Schlossberg loop/]);
  await openSlot(page, 'way');
  await expect(summit.getByRole('button', { name: 'Must' })).toHaveAttribute('aria-pressed', 'true');

  await summit.getByRole('button', { name: 'Summit' }).click();
  await openSlot(page, 'hike');
  await page.locator('#search-panel').getByRole('button', { name: /^Shape/ }).click();
  await page.getByRole('button', { name: 'Loop', exact: true }).click();
  await expect(rows).toHaveText([/Westweg 2/]);
  await expect(slotText(page, 'hike')).toHaveText('There and back');
  // Escape closes the panel and leaves the search as it is.
  await page.keyboard.press('Escape');
  await expect(page.locator('#search-panel')).toHaveCount(0);
  // A row opens the card of its route over the map, with the facts of its shape and the link to the page of the route.
  // The view is the map's own only after the map loads: before it, the view of the history can still change.
  await expect(page.locator('.routes-map')).toHaveAttribute('data-loaded', 'true');
  await page.waitForTimeout(500);
  const wide = viewOf(page);
  await rows.filter({ hasText: 'Westweg 2' }).click();
  const card = page.getByRole('region', { name: 'Westweg 2' });
  await expect(card).toContainText('There and back, 2.5 km each way');
  await expect(card.getByRole('link', { name: 'Westweg 2' })).toHaveAttribute('href', '#/signed/9-267-177/202');
  // The card with its long text leaves a part of the map free, and the map moves the route there.
  await expect.poll(() => viewOf(page)).not.toEqual(wide);
  const kept = await page.evaluate(() => new Promise(done => setTimeout(() => done(history.state.explore), 500)));
  expect(kept.query).toBe('shapes=back,liftDown,liftUp');

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

  await expect(page).toHaveURL(/#\/explore\?shapes=back,liftDown,liftUp&view=/);
  await expect(currentRow(page)).toContainText('Westweg 2');
  await expect(card).toBeVisible();
  // Until the map loads, the page keeps the view of the history; after it, the view is the map's own.
  await expect(page.locator('.routes-map')).toHaveAttribute('data-loaded', 'true');
  const back = await page.evaluate(() => new Promise(done => setTimeout(() => done(history.state.explore), 500)));
  expect([back.query, back.selected, back.shape]).toEqual([kept.query, '202', 'back']);
  for (const [i, value] of kept.view.entries()) expect(back.view[i]).toBeCloseTo(value, 4);
  // Escape closes the card.
  await page.keyboard.press('Escape');
  await expect(card).toHaveCount(0);

  // Explore searches hikes only: the bike kinds are greyed out.
  await openSlot(page, 'hike');
  await page.locator('#search-panel').getByRole('button', { name: /^Kind/ }).click();
  await expect(page.getByRole('radio', { name: 'Hiking' })).toBeChecked();
  await expect(page.getByRole('radio', { name: 'Mountain bike' })).toBeDisabled();

  // The cross empties the search, and "Undo" brings it back.
  await page.getByRole('button', { name: 'Clear the search' }).click();
  await expect(rows).toHaveCount(2);
  await page.getByRole('button', { name: 'Undo' }).click();
  await expect(rows).toHaveText([/Westweg 2/]);
  expect(errors).toEqual([]);
});

test('the text field sends each typed phrase to its slot, takes one suggestion with Tab and keeps the words that it does not know', async ({ page }) => {
  const errors = [], asked = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('request', request => { if (request.url().endsWith('/query') && request.method() === 'POST') asked.push(request.postDataJSON().request); });
  await findFreiburg(page, 1440, 900);
  // A typed name asks the server for places of that name only, so the server runs no model.
  expect(asked).toEqual([{ type: 'place', name: 'Freiburg' }]);
  const rows = rowsOf(page), field = page.getByRole('combobox', { name: 'Say what you want' });
  await expect(rows).toHaveCount(2);

  // The words stay while the rider types, and the field marks each phrase that it understands.
  await field.fill('loop with a summit, sunny');
  await expect(page.locator('.mirror .m-value')).toHaveText(['loop', 'summit']);
  // The first line of the list names the slot of each marked phrase.
  await expect(page.locator('.reading .to')).toHaveText([/^Hike\s*Loop$/, /^On the way\s*Summit$/]);
  // Enter never takes a place of the server: each marked phrase goes to its slot, and the unknown word stays.
  await expect(page.getByRole('option', { name: /^Freiburg/ })).toBeVisible();
  await field.press('Enter');
  await expect(slotText(page, 'hike')).toHaveText('Loop');
  await expect(slotText(page, 'way')).toHaveText('Summit');
  await expect(rows).toHaveText([/Schlossberg loop/]);
  await expect(field).toHaveValue('sunny ');
  await expect(page.getByText('Not understood: “sunny”. It changes nothing.')).toBeVisible();

  // Tab takes one suggestion for the words in progress, and the other words stay.
  await field.fill('sunny 2');
  await expect(page.getByRole('option').first()).toHaveText(/Under 2\skm\s*Hike/);
  await field.press('Tab');
  await expect(slotText(page, 'hike')).toHaveText(/^Loop · Under 2\skm$/);
  await expect(field).toHaveValue('sunny ');
  await expect(rows).toHaveText([/Schlossberg loop/]);

  // A name that begins a place of the server is not unknown: the list says what a click on the place does.
  await field.fill('Freib');
  await expect(page.locator('.search-panel.list')).toHaveText(/^\s*Move the map to\s*Freiburg\s*city · Testland\s*$/);

  // A click on the first row sends what it shows, also while a place is the active row: the words in progress stay, and
  // the map does not move.
  await field.fill('forest Freib');
  const marked = page.getByRole('option', { name: 'On the way: Mostly forest' }), view = viewOf(page);
  await expect(marked).toBeVisible();
  await field.press('ArrowDown');
  await field.press('ArrowDown');
  await expect(page.getByRole('option', { name: /^Freiburg/ })).toHaveAttribute('aria-selected', 'true');
  await marked.click();
  await expect(slotText(page, 'way')).toHaveText('Summit · Mostly forest');
  await expect(field).toHaveValue('Freib');
  expect(viewOf(page)).toEqual(view);
  expect(errors).toEqual([]);
});

test('the name on the card of a hike opens the page of the route, and Back and "‹ Explore" open the card again', async ({ page }) => {
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  const open = await openCard(page, 'Westweg 2', 1440, 900);
  // A nice wish is the last search of the browser. The tick is a must first, which closes the card of the stage.
  await openSlot(page, 'way');
  await page.locator('.wish', { hasText: 'Summit' }).getByRole('button', { name: 'Summit' }).click();
  await expect(open).toHaveCount(0);
  await page.locator('.wish', { hasText: 'Summit' }).getByRole('button', { name: 'Must' }).click();
  await page.keyboard.press('Escape');
  // A row shows each nice wish, with a tick when it meets it.
  await expect(rowsOf(page)).toHaveText([/Large car park\s*Summit\s*$/, /No car park\s*no Summit\s*$/]);
  await rowsOf(page).filter({ hasText: 'Westweg 2' }).click();
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
    await expect(currentRow(page)).toContainText(name);
    await expect(page.getByRole('region', { name })).toBeVisible();
    await expect(slotText(page, 'way')).toHaveText('Summit if possible');
  }

  // A last search that leaves the route out gives way to the default search, so the card of the route opens.
  await openSlot(page, 'way');
  await page.locator('.wish', { hasText: 'Summit' }).getByRole('button', { name: 'Must' }).click();
  await expect(page).toHaveURL(/must=peak/);
  await page.goto('/planner.html#/signed/9-267-177/202');
  await page.reload();
  await page.getByRole('link', { name: '‹ Explore' }).click();
  await expect(currentRow(page)).toContainText('Westweg 2');
  await expect(slotText(page, 'way')).toHaveText('Anything');

  // So does a last search with a start point far from the route, once the answer for the view of the route has come.
  await page.evaluate(() => localStorage.setItem('explore-day-query', 'start=10.2,49.6'));
  await page.goto('/planner.html#/signed/9-267-177/101');
  await page.reload();
  await page.getByRole('link', { name: '‹ Explore' }).click();
  await expect(currentRow(page)).toContainText('Schlossberg loop');
  await expect(slotText(page, 'from')).toHaveText('Anywhere on the map');

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
  await expect(slotText(page, 'from')).toHaveText('Start point · By car, up to 40 km');
  await expect(rowsOf(page)).toHaveText([/Schlossberg loop/]);
  expect(errors).toEqual([]);
});

test('the map asks for the hikes that pass a place, pins a car park as the only start, and takes a start point', async ({ page, context }) => {
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await findFreiburg(page, 1440, 1000);
  const rows = rowsOf(page);
  await expect(rows).toHaveCount(2);
  await expect(page.locator('.routes-map')).toHaveAttribute('data-loaded', 'true');

  // A selected row moves the map only to show its route over the card: it pans or zooms out, never in, so the list keeps
  // its rows. Whether the card covers the loop depends on the height of its text, so the test asks only for that rule.
  await expect(page).toHaveURL(/view=/);
  await page.waitForTimeout(500);
  const shown = viewOf(page), span = ([west, south, east, north]) => [east - west, north - south];
  await rows.filter({ hasText: 'Schlossberg loop' }).click();
  await expect(page.getByRole('region', { name: 'Schlossberg loop' })).toBeVisible();
  await page.waitForTimeout(500);
  span(viewOf(page)).forEach((size, i) => expect(size).toBeGreaterThanOrEqual(span(shown)[i] - 1e-4));
  await expect(rows).toHaveCount(2);
  await page.getByRole('button', { name: 'Close' }).click();

  // A summit shows on the map while Summit is a wish; a click on it asks for the hikes that pass it.
  await openSlot(page, 'way');
  await page.locator('.wish', { hasText: 'Summit' }).getByRole('button', { name: 'Summit' }).click();
  await page.locator('.wish', { hasText: 'Summit' }).getByRole('button', { name: 'Must' }).click();
  await page.keyboard.press('Escape');
  const summit = await pointOf(page, 7.86, 47.995);
  // The tip follows the pointer: it moves until the marker has drawn.
  await expect(async () => {
    await page.mouse.move(summit.x, summit.y + 1);
    await page.mouse.move(summit.x, summit.y);
    await expect(page.locator('.tip')).toHaveText(/^Schlossberg 460\sm\s*1 hike passes here\s*Click: only hikes that pass here$/, { timeout: 500 });
  }).toPass();
  await page.mouse.click(summit.x, summit.y);
  await expect(rows).toHaveText([/Schlossberg loop/]);
  await expect(slotText(page, 'way')).toHaveText('Summit if possible · Passes Schlossberg');
  await expect(page).toHaveURL(/must=via:n1&/);
  // The tip after the click says what the next click does.
  await expect(async () => {
    await page.mouse.move(summit.x, summit.y + 1);
    await page.mouse.move(summit.x, summit.y);
    await expect(page.locator('.tip')).toHaveText(/1 hike passes here\s*Click: show all hikes again$/, { timeout: 500 });
  }).toPass();
  await page.mouse.click(summit.x, summit.y);
  await expect(rows).toHaveCount(2);

  // The car park that the loop names is a marker; a click pins it as the only start, and Unpin gives the others back.
  const park = await pointOf(page, 7.85, 47.989);
  await page.mouse.click(park.x, park.y);
  await expect(rows).toHaveText([/Schlossberg loop/]);
  await expect(page).toHaveURL(/pin=w2&/);
  await openSlot(page, 'from');
  await expect(page.getByText('Pinned on the map')).toBeVisible();
  // The map fits the walk and the last walk around the pin: 4 km on each side.
  await expect.poll(() => { const [west, south, east, north] = viewOf(page); return west < 7.85 && 7.85 < east && Math.abs(south - 47.953) < 0.005 && Math.abs(north - 48.025) < 0.005; }).toBe(true);
  await page.getByRole('button', { name: 'Unpin' }).click();
  await expect(rows).toHaveCount(2);

  // "Pick on the map" closes the panel and takes the next click as the start point.
  await page.getByRole('button', { name: 'Pick on the map' }).click();
  await expect(page.locator('#search-panel')).toHaveCount(0);
  await expect(page.getByText('Click the map where you start. Esc cancels.')).toBeVisible();
  const picked = await pointOf(page, 7.9, 47.985);
  await page.mouse.click(picked.x, picked.y);
  await expect(slotText(page, 'from')).toHaveText(/^Start point · On foot, up to 1\skm$/);
  await expect(rows).toHaveText([/Westweg 2/]);
  await expect(page).toHaveURL(/start=/);
  const [lon, lat] = new URL(page.url()).hash.match(/start=([\d.]+,[\d.]+)/)[1].split(',').map(Number);
  expect(lon).toBeCloseTo(7.9, 2);
  expect(lat).toBeCloseTo(47.985, 2);

  // The start point takes no click: it asks for nothing.
  await expect.poll(() => { const [west, south, east, north] = viewOf(page); return Math.abs((south + north) / 2 - lat) < 0.002 && west < lon && lon < east; }).toBe(true);
  const start = await pointOf(page, lon, lat);
  await page.mouse.click(start.x, start.y);
  await page.waitForTimeout(300);
  await expect(page).not.toHaveURL(/via:/);
  await expect(rows).toHaveText([/Westweg 2/]);

  // "My location" asks the browser: a refusal gives a short note, a position the start point.
  await openSlot(page, 'from');
  await page.getByRole('button', { name: 'Remove the start point' }).click();
  await page.getByRole('button', { name: 'My location' }).click();
  await expect(page.getByText('The browser does not share your location.')).toBeVisible();
  await context.grantPermissions(['geolocation']);
  await context.setGeolocation({ longitude: 7.851, latitude: 47.991 });
  await page.getByRole('button', { name: 'My location' }).click();
  await expect(page).toHaveURL(/start=7\.851,47\.991&/);
  await expect(rows).toHaveText([/Schlossberg loop/]);

  // A row whose route runs out of the view pans or zooms the map out until the route shows, never in.
  await page.goto('/planner.html#/explore?view=7.875,47.975,7.895,47.99');
  await page.reload();
  await expect(rows.filter({ hasText: 'Westweg 2' })).toHaveCount(1);
  await expect(page.locator('.routes-map')).toHaveAttribute('data-loaded', 'true');
  await page.waitForTimeout(500);
  const height = ([, south, , north]) => north - south, before = height(viewOf(page));
  await rows.filter({ hasText: 'Westweg 2' }).click();
  await expect.poll(() => viewOf(page)[3]).toBeGreaterThan(48);
  expect(height(viewOf(page))).toBeGreaterThanOrEqual(before - 1e-4);
  expect(errors).toEqual([]);
});

test('an empty list names the changes that help, and its "Clear the search" has the Undo of the bar', async ({ page }) => {
  await stub(page);
  await page.goto('/planner.html#/explore?must=peak&shapes=back&view=7.8,47.96,7.92,48.02');
  const list = listOf(page), rows = rowsOf(page);
  await expect(list.getByRole('heading')).toHaveText('0 hikes');
  await expect(list.locator('.helps button')).toHaveText([/^Summit as a wish only\s*1 hike$/, /^Loop too\s*1 hike$/]);
  await list.getByRole('button', { name: 'Clear the search' }).click();
  await expect(rows).toHaveCount(2);
  await page.getByRole('button', { name: 'Undo' }).click();
  await expect(rows).toHaveCount(0);
  // A change takes its button away: the first row of the new list has the focus.
  await list.getByRole('button', { name: /^Loop too/ }).press('Enter');
  await expect(rows).toHaveText([/Schlossberg loop/]);
  await expect(rows.first()).toBeFocused();
});

/** Opens the panel of a slot of the search bar, if it is not open. */
async function openSlot(page, id) {
  const slot = page.locator(`.search-bar [data-slot="${id}"]`);
  if (await slot.getAttribute('aria-expanded') !== 'true') await slot.click();
  await expect(page.locator('#search-panel')).toBeVisible();
}

/** The words of a slot of the search bar. */
const slotText = (page, id) => page.locator(`.search-bar [data-slot="${id}"] strong`);

/** The list beside the map, its rows, and the row of the selected route. */
const listOf = page => page.locator('.cards > .list');
const rowsOf = page => listOf(page).locator('.row');
const currentRow = page => listOf(page).locator('.row[aria-current="true"]');

/** The map view in the page address. */
const viewOf = page => new URL(page.url()).hash.match(/view=([-\d.,]+)/)[1].split(',').map(Number);

/** The point of the page at `lon`, `lat` on the Explore map, from the view in the page address. */
async function pointOf(page, lon, lat) {
  await expect(page).toHaveURL(/view=/);
  const [west, south, east, north] = viewOf(page);
  const box = await page.locator('.routes-map canvas').boundingBox();
  const y = latitude => Math.log(Math.tan(Math.PI / 4 + latitude * Math.PI / 360));
  return { x: box.x + (lon - west) / (east - west) * box.width, y: box.y + (y(north) - y(lat)) / (y(north) - y(south)) * box.height };
}

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
  expect(state).toEqual({ view: [7.88, 47.98, 7.91, 48].map(value => expect.closeTo(value, 6)), tab: 'long', trail: 300 });
  await expect(page).toHaveURL(/#\/explore$/);

  await page.goto('/planner.html#/trail/1');
  await expect(page.getByText('This trail is no longer in the map data.')).toBeVisible();
  expect(errors).toEqual([]);
});

/** Opens the card of the day hike `name` in a window of `width` × `height` px. */
async function openCard(page, name, width, height) {
  await findFreiburg(page, width, height);
  await rowsOf(page).filter({ hasText: name }).click();
  return page.getByRole('region', { name });
}

async function findFreiburg(page, width, height) {
  await page.setViewportSize({ width, height });
  await stub(page);
  await page.goto('/planner.html#/explore');
  await page.getByRole('combobox', { name: 'Say what you want' }).fill('Freiburg');
  await page.getByRole('option', { name: /^Freiburg/ }).click();
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

test('on a phone, the card of a hike with a long text and no profile stays on the screen, and its text scrolls', async ({ page }) => {
  const card = await openCard(page, 'Westweg 2', 390, 844);
  // The card is never higher than the screen less 96 px.
  await expect.poll(() => card.evaluate(node => [!node.querySelector('.profile'), node.getBoundingClientRect().height <= innerHeight - 96]))
    .toEqual([true, true]);
  await textScrolls(page, card, 1);
});

// 844 px wide has the layout of a computer, and 667 px wide that of a phone.
for (const [width, height] of [[844, 390], [667, 375]]) {
  test(`in a window of ${width} × ${height} px, the card of a hike with a profile shows no strip, and two lines of its text that scroll`, async ({ page }) => {
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
