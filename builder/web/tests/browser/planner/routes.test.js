import { expect, test } from '@playwright/test';
import { CORS_PREFLIGHT, respond } from './stubs.js';

const drawn = (id, kind, lat, km) => ({
  id, kind, label: id, coordinate: [8, lat],
  ...kind !== 'start' && { leg: 'drawn', drawn: [[8, lat - km / 222, 400], [8, lat, 500]] },
});
const summary = (lengthM, days) => ({ line: [8, 48, 8, 48.2], heights: [300, 700], bounds: [8, 48, 8, 48.2], lengthM, climbM: 400, highestM: 700, seconds: 4000, bike: 'touring', days });
const trip = days => ({ mode: 'route', bike: 'touring', days, routeOrder: [], points: [drawn('Start', 'start', 48), drawn('Finish', 'finish', 48.2, 22)] });

// A named route that waits, a plan with no name on the device with an older line, and a route that is not a plan.
const routes = [
  { id: 'lake', revision: 0, name: 'Lake test', updatedAt: 3, trip: trip(1), summary: summary(22_239, 1) },
  { id: 'long', revision: 0, name: '', updatedAt: 2, trip: trip(3), summary: summary(150_000, 3) },
  { id: 'broken', revision: 0, name: '', updatedAt: 1, trip: { nope: true }, summary: summary(22_239, 1) },
];

test('the Routes page lists the routes, filters them with a chip, shows the selected one and its state', async ({ page }) => {
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.route(url => url.hostname !== '127.0.0.1', request => {
    if (request.request().method() === 'OPTIONS') return request.fulfill(CORS_PREFLIGHT);
    const response = respond(request.request());
    return response ? request.fulfill(response) : request.abort();
  });
  await page.goto('/planner.html#/routes');
  await expect(page.getByText('No routes yet.')).toBeVisible();

  await page.goto('/planner.html?cover=none');
  await page.waitForFunction(() => window.coverTest);
  await page.evaluate(async records => {
    const { library } = window.coverTest;
    for (const record of records) await library.saveRoute(record);
    await library.setState('waiting', ['lake']);
    await library.setState('sent', { long: { device: 'card:7', key: 'older' } });
    localStorage.setItem('obc-device', JSON.stringify({ serial: '00AB', storeId: 'card', at: null, map: null, routes: 1, bytes: { map: 0, routes: 0, rides: 0, other: 0 }, came: null, skipped: [] }));
  }, routes);

  await page.goto('/planner.html#/routes');
  const list = page.locator('ul').filter({ hasText: 'Lake test' });
  await expect(page.getByRole('radio', { name: 'Planned 3' })).toBeChecked();
  await expect(list.getByRole('button')).toHaveText([/Lake test.*Waits/, /Start → Finish.*3 days · 150\skm.*Changed/, /Route that cannot be read/]);

  await list.getByRole('button', { name: 'Route that cannot be read' }).click();
  const card = page.getByRole('region', { name: 'Route that cannot be read' });
  await expect(card.getByRole('link', { name: 'Route that cannot be read' })).toHaveAttribute('href', '#/route/broken');
  await expect(card.locator('.buttons button')).toHaveCount(0);
  await expect(card.getByRole('link', { name: 'Open in planner' })).toHaveCount(0);

  await list.getByRole('button', { name: /Start → Finish/ }).click();
  const plan = page.getByRole('region', { name: 'Start → Finish' });
  await expect(plan.getByRole('link', { name: 'Open in planner' })).toHaveAttribute('href', '#/planner/long');
  await expect(plan).toContainText('Changed since it was sent');
  await plan.getByRole('button', { name: 'Send to device' }).click();
  await expect(plan).toContainText('Waits for the device');
  await expect(list.getByRole('button', { name: /Start → Finish/ })).toContainText('Waits');
  await plan.getByRole('button', { name: 'Do not send' }).click();
  await expect(plan).toContainText('Changed since it was sent');

  // A click anywhere on the card opens the route; a click on the empty map, Escape and the button of the card close it.
  // The name is the link that covers the card: a click on the card text lands on it, as a click of the rider does.
  await plan.getByText('Touring bike · 3 days').click({ force: true });
  await expect(page).toHaveURL(/#\/route\/long$/);
  await page.goBack();
  const row = list.getByRole('button', { name: /Start → Finish/ });
  await row.click();
  const map = page.locator('.map');
  await expect(map.locator('.maplibregl-canvas')).toBeVisible();
  // The map shows the relief, so the credit of the terrain opens with the credit of the map.
  const more = page.locator('.more-credits p');
  await expect(more).toBeHidden();
  await map.getByLabel('Toggle attribution').click();
  await expect(more).toBeVisible();
  await expect(more).toContainText('Terrain test data');
  const box = await map.boundingBox();
  await page.mouse.click(box.x + 40, box.y + box.height / 2);
  await expect(plan).toHaveCount(0);
  await row.click();
  await page.keyboard.press('Escape');
  await expect(plan).toHaveCount(0);
  await row.click();
  await plan.getByRole('button', { name: 'Close' }).click();
  await expect(plan).toHaveCount(0);
  await expect(page).toHaveURL(/#\/routes$/);

  // The lengths 22 and 150 km give the slider the step 10 km: the upper handle at 100 km keeps the short route.
  await page.getByText('+ Length').click();
  await page.getByRole('slider', { name: 'Length, to' }).fill('100');
  await expect(page.getByText('1 of 3 routes')).toBeVisible();
  await page.locator('summary', { hasText: 'Under' }).click();
  await expect(list.getByRole('button')).toHaveText([/Lake test/]);
  await expect(plan).toHaveCount(0);

  // Back from a route brings the search again.
  await list.getByRole('button', { name: /Lake test/ }).click();
  await page.getByRole('region', { name: 'Lake test' }).getByRole('link', { name: 'Lake test' }).click();
  await expect(page.getByRole('heading', { name: 'Lake test', level: 1 })).toBeVisible();
  await page.goBack();
  await expect(page.getByText('1 of 3 routes')).toBeVisible();

  await page.getByRole('link', { name: 'New route' }).click();
  await expect(page).toHaveURL(/#\/planner$/);
  await expect(page.locator('.maplibregl-canvas').first()).toBeVisible();
  expect(errors).toEqual([]);
});

// The boxes of the parts of the page with a selected route, as x, y, width and height in page pixels. A screenshot
// cannot hold them: the text antialiasing differs between two systems. At the width of a phone the card is fixed at
// the bottom of the screen, which is 844 px high. With save-data on, the cover job makes no
// picture, so the card shows the line on a plain ground.
const PARTS = { header: '.routes-page > header', chips: '.chips', rows: '.list .row', card: '.selected .card',
  picture: '.selected .picture', text: '.selected .text', profile: '.selected .profile', buttons: '.selected .buttons' };
const LAYOUT = {
  1440: { header: [20, 62, 1400, 36], chips: [31, 173, 458, 24], rows: [[31, 205, 458, 114], [31, 319, 458, 114], [31, 433, 458, 114]],
    card: [527, 819, 880, 222], picture: [538, 830, 300, 200], text: [858, 830, 367, 75], profile: [858, 917, 538, 112], buttons: [1245, 830, 151, 78] },
  1024: { header: [20, 62, 984, 36], chips: [31, 173, 358, 24], rows: [[31, 205, 358, 114], [31, 319, 358, 114], [31, 433, 358, 114]],
    card: [427, 826, 564, 214], picture: [438, 837, 190, 126], text: [644, 837, 336, 69], profile: [438, 974, 542, 56], buttons: [644, 930, 336, 34] },
  390: { header: [16, 60, 358, 80], chips: [27, 214, 336, 24], rows: [[27, 246, 336, 94], [27, 340, 336, 94], [27, 434, 336, 94]],
    card: [12, 616, 366, 216], picture: [23, 627, 120, 80], text: [159, 627, 208, 69], profile: [23, 765, 344, 56], buttons: [23, 719, 242, 34] },
};
const layout = page => page.evaluate(parts => {
  const box = node => { const r = node.getBoundingClientRect(); return r.width ? [r.x, r.y + scrollY, r.width, r.height] : null; };
  return Object.fromEntries(Object.entries(parts).map(([key, selector]) =>
    [key, key === 'rows' ? [...document.querySelectorAll(selector)].map(box) : box(document.querySelector(selector))]));
}, PARTS);
/** Each number within 1 px of the expected one. */
const near = (actual, expected) => expected === null ? actual === null
  : Array.isArray(expected) && actual?.length === expected.length && expected.every((value, i) => Array.isArray(value) ? near(actual[i], value) : Math.abs(actual[i] - value) <= 1);

test('the Routes page with a selected route, at the width of a computer and of a phone', async ({ page }) => {
  await page.addInitScript(() => Object.defineProperty(NetworkInformation.prototype, 'saveData', { get: () => true }));
  await page.route(url => url.hostname !== '127.0.0.1', request => {
    if (request.request().method() === 'OPTIONS') return request.fulfill(CORS_PREFLIGHT);
    const response = respond(request.request());
    return response ? request.fulfill(response) : request.abort();
  });
  await page.goto('/planner.html?cover=none');
  await page.waitForFunction(() => window.coverTest);
  await page.evaluate(async records => {
    const { library } = window.coverTest;
    for (const record of records) await library.saveRoute(record);
    await library.setState('waiting', ['lake']);
  }, routes);

  await page.goto('/planner.html#/routes');
  await page.locator('.list .row', { hasText: 'Start → Finish' }).click();
  await expect(page.getByRole('region', { name: 'Start → Finish' }).locator('svg polyline')).toHaveCount(1);
  // The pale fills are below the threshold of the pictures, so the test reads them.
  await expect(page.locator('.list .row[aria-current="true"]')).toHaveCSS('background-color', 'rgb(231, 236, 223)');
  await expect(page.locator('.list .thumb .cover').first()).toHaveCSS('background-color', 'rgba(0, 0, 0, 0)');
  await page.evaluate(() => document.fonts.ready);
  for (const width of [1440, 1024, 390]) {
    await page.setViewportSize({ width, height: width === 390 ? 844 : 1100 });
    await expect.poll(async () => { const actual = await layout(page); return Object.keys(PARTS).filter(key => !near(actual[key], LAYOUT[width][key])); },
      { message: `parts out of place at ${width} px` }).toEqual([]);
  }
});

const year = new Date().getFullYear();
const lake = [8, 48, 8.1, 48, 8.1, 48.1, 8, 48.1, 8, 48];
function ride(name, line, lengthM, month, day, bike = 'road', at = year) {
  const start = new Date(at, month, day, 9).getTime() / 1000;
  const xs = line.filter((_, i) => i % 2 === 0), ys = line.filter((_, i) => i % 2);
  return {
    key: new Date(start * 1000).toISOString().replace(/[-:]|\.\d+/g, ''), name, source: 'file', format: 'gpx', maxHr: null, ftp: null,
    summary: { line, heights: xs.map(() => 400), bounds: [Math.min(...xs), Math.min(...ys), Math.max(...xs), Math.max(...ys)],
      lengthM, climbM: 100, highestM: 400, seconds: 3600, bike, start, movingS: 3000 },
  };
}

test('the ridden view shows a trip, a group of rides on a route, a ride taken out of it, and the year', async ({ page }) => {
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.route(url => url.hostname !== '127.0.0.1', request => {
    if (request.request().method() === 'OPTIONS') return request.fulfill(CORS_PREFLIGHT);
    const response = respond(request.request());
    return response ? request.fulfill(response) : request.abort();
  });
  await page.goto('/planner.html#/routes/ridden');
  await expect(page.getByText('No rides yet.')).toBeVisible();
  await expect(page.getByRole('radio', { name: 'Ridden 0' })).toBeChecked();

  // Four rides of the lake route and a trip of two days along the coast.
  const rides = [...[0, 1, 2, 3].map(month => ride(`Lake ${month + 1}`, lake, 37_000, month, 5)),
    ride('Coast, day 1', [9, 47, 9.5, 47], 38_000, 5, 1), ride('Coast, day 2', [9.5, 47, 10, 47], 38_000, 5, 2)];
  await page.goto('/planner.html?cover=none');
  await page.waitForFunction(() => window.coverTest);
  await page.evaluate(async ({ rides, route }) => {
    const { library } = window.coverTest;
    await library.saveRoute(route);
    for (const ride of rides) await library.addRide(ride, new Blob(['ride']));
  }, { rides, route: { id: 'lake', revision: 0, name: 'Lake test', updatedAt: 1, trip: trip(1), summary: { ...rides[0].summary, days: 1 } } });

  await page.goto('/planner.html#/routes/ridden');
  await expect(page.getByRole('radio', { name: 'Ridden 6' })).toBeChecked();
  const list = page.locator('.list ul');
  await expect(list.locator('li')).toHaveText([/Coast, day 1 1 to 2 Jun \d+ · 2 days · 76.0\skm/, /Lake test 37.0\skm · last on 5 Apr \d+ 4 rides ›/]);
  // The button that opens the group lies on the hidden room of its row.
  const [open, room] = await Promise.all([list.locator('button.open'), list.locator('.room')].map(part => part.boundingBox()));
  expect(Math.abs(open.x - room.x)).toBeLessThan(1);
  expect(Math.abs(open.width - room.width)).toBeLessThan(1);
  await expect(page.locator('.totals')).toHaveText(new RegExp(`${year} · 224\\skm · 6 rides`));

  // The card of a trip opens its first day; the card of a group takes its newest ride out.
  await list.locator('.row', { hasText: 'Coast, day 1' }).click();
  const coast = page.getByRole('region', { name: 'Coast, day 1' });
  await expect(coast).toContainText('2 days');
  await expect(coast.getByRole('link', { name: 'Coast, day 1' })).toHaveAttribute('href', `#/ride/${rides[4].key}`);
  await list.locator('.row', { hasText: 'Lake test' }).click();
  await page.getByRole('region', { name: 'Lake test' }).getByRole('button', { name: 'Take out of the group' }).click();
  await expect(list.locator('li')).toHaveText([/Coast, day 1/, /Lake 4 5 Apr \d+ · 37.0\skm · 100\sm up/, /Lake test.*last on 5 Mar.*3 rides ›/]);

  await list.getByRole('button', { name: '3 rides ›' }).click();
  await expect(list.locator('li')).toHaveText([/Lake 3/, /Lake 2/, /Lake 1/]);
  // In an open group, each ride can leave the group.
  await list.locator('.row', { hasText: 'Lake 1' }).click();
  await page.getByRole('region', { name: 'Lake 1' }).getByRole('button', { name: 'Take out of the group' }).click();
  await expect(list.locator('li')).toHaveText([/Lake 3/, /Lake 2/]);
  await page.getByRole('button', { name: '‹ All rides' }).click();
  await expect(list.locator('li')).toHaveCount(4);

  await page.getByRole('radio', { name: 'Planned 1' }).click();
  await expect(page).toHaveURL(/#\/routes$/);
  await page.locator('ul').getByRole('button', { name: /Lake test/ }).click();
  await expect(page.getByRole('region', { name: 'Lake test' })).toContainText('ridden 2 times');
  expect(errors).toEqual([]);
});

test('the line of the year opens the statistics: the years, the months of a year, on foot apart, and the longest ride', async ({ page }) => {
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.route(url => url.hostname !== '127.0.0.1', request => {
    if (request.request().method() === 'OPTIONS') return request.fulfill(CORS_PREFLIGHT);
    const response = respond(request.request());
    return response ? request.fulfill(response) : request.abort();
  });
  const rides = [ride('Old loop', lake, 50_000, 3, 5, 'road', year - 1), ride('Long day', lake, 80_000, 4, 5), ride('Walk', lake, 12_000, 5, 5, 'hiking')];
  await page.goto('/planner.html?cover=none');
  await page.waitForFunction(() => window.coverTest);
  await page.evaluate(async rides => {
    for (const ride of rides) await window.coverTest.library.addRide(ride, new Blob(['ride']));
  }, rides);

  // The line never adds the ride by bike and the hike.
  await page.goto('/planner.html#/routes/ridden');
  await expect(page.locator('.totals')).toHaveText(new RegExp(`${year} · 80.0\\skm · 1 ride · 12.0\\skm · 1 hike`));
  await page.locator('.totals').click();
  const statistics = page.getByRole('dialog', { name: 'Statistics' });
  const bars = statistics.getByRole('list').getByRole('listitem'), distance = statistics.getByRole('radio', { name: /Distance/ });
  await expect(bars).toHaveText([new RegExp(`50.*${year - 1}`), new RegExp(`80.*${year}`)]);
  await expect(distance).toHaveText(/130\skm/);

  // A year shows its 12 months; "All years" and Escape go back, and the window stays open.
  await statistics.getByRole('button', { name: new RegExp(`^${year - 1}: 50.0\\skm$`) }).click();
  await expect(bars).toHaveCount(12);
  await expect(distance).toHaveText(/50.0\skm/);
  await statistics.getByRole('button', { name: 'All years' }).click();
  await expect(bars).toHaveCount(2);
  await statistics.getByRole('button', { name: `${year - 1}` }).click();
  await page.keyboard.press('Escape');
  await expect(bars).toHaveCount(2);

  // On foot has one year, so its months show at once.
  await statistics.getByRole('radio', { name: 'On foot' }).click();
  await expect(distance).toHaveText(/12.0\skm/);
  await expect(bars).toHaveCount(12);
  await expect(statistics.getByRole('button', { name: 'All years' })).toHaveCount(0);
  expect(errors).toEqual([]);

  await statistics.getByRole('radio', { name: 'By bike' }).click();
  await statistics.getByRole('link', { name: /Longest ride/ }).click();
  await expect(page).toHaveURL(new RegExp(`#/ride/${rides[1].key}$`));
  await expect(statistics).toHaveCount(0);
});
