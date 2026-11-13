import { expect, test } from '@playwright/test';
import { CORS_PREFLIGHT, respond } from './stubs.js';

const HOME_VIEWS = 'https://maps.openbikecomputer.com/home-views';
const NOON = new Date(2026, 10, 13, 12);
const DAY = 86_400_000;
const PIXEL = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==', 'base64');
const view = id => ({ id, name: `View ${id}`, region: 'Central Switzerland', lat: 46.8, lon: 8.4, sunrise: '07:00', sunset: '17:00', night: false });
const manifest = set => ({ format: 1, set, date: '2026-10-15', width: 2560, height: 1440, credits: [`© Data of ${set}`], views: ['a', 'b', 'c'].map(view) });
// A place with a box, inside the area of the map of the test build.
const PLACE = { name: 'Testdorf', kind: 'village', lon: 8.05, lat: 48.05, west: 8.04, south: 48.04, east: 8.06, north: 48.06, region: 'Testland' };

/** Answers the hosts of the planner build at a fixed clock; `homeViews` answers the home views, else they fail. The search
 * service finds one place. */
async function start(page, homeViews) {
  await page.clock.setFixedTime(NOON);
  await page.route(url => url.hostname !== '127.0.0.1', route => {
    const request = route.request();
    if (request.method() === 'OPTIONS') return route.fulfill(CORS_PREFLIGHT);
    if (homeViews && request.url().startsWith(HOME_VIEWS)) return homeViews(route, request.url().slice(HOME_VIEWS.length));
    if (request.url().endsWith('/query')) {
      return route.fulfill({ json: { type: 'places', request: { type: 'place' }, results: [PLACE] }, headers: { 'access-control-allow-origin': '*' } });
    }
    const response = respond(request);
    return response ? route.fulfill(response) : route.abort();
  });
}

async function fill(page, { routes = [], rides = [], active = null }) {
  await page.goto('/planner.html?cover=none');
  await page.waitForFunction(() => window.coverTest);
  await page.evaluate(async ({ routes, rides, active }) => {
    const { library } = window.coverTest;
    for (const route of routes) await library.saveRoute(route);
    for (const ride of rides) await library.saveRide(ride);
    await library.setState('active', active);
  }, { routes, rides, active });
}

const summary = { line: [8, 48, 8.1, 48.1], heights: [400, 500], bounds: [8, 48, 8.1, 48.1], lengthM: 62_000, climbM: 400, highestM: 500, seconds: 9000, bike: 'gravel' };
const point = (id, kind, lat) => ({ id, kind, label: id, coordinate: [8, lat], ...kind !== 'start' && { leg: 'drawn', drawn: [[8, lat - 0.1, 400], [8, lat, 500]] } });
const trip = { mode: 'route', bike: 'gravel', days: 1, routeOrder: [], points: [point('Start', 'start', 48), point('Finish', 'finish', 48.1)] };
const lake = { id: 'lake', revision: 0, name: 'Lake test', updatedAt: NOON.getTime() - DAY, trip, summary: { ...summary, days: 1 } };
const ride = (key, name, start) => ({ key, name, source: 'file', format: 'gpx', maxHr: null, ftp: null, summary: { ...summary, start: start.getTime() / 1000, movingS: 9000 } });
const rides = [ride('20261109T090000Z', 'Kaiserstuhl', new Date(2026, 10, 9, 10)), ride('20261001T090000Z', 'Schauinsland', new Date(2026, 9, 1, 10))];

test('a first visit with no home views shows the slogan and the previews on a plain ground, also for an unknown page', async ({ page }) => {
  await start(page);
  for (const path of ['/planner.html', '/planner.html#/nowhere']) {
    await page.goto(path);
    await expect(page.getByRole('heading', { name: '[Slogan goes here]' })).toBeVisible();
    await expect(page.getByRole('region', { name: 'What you can do here' }).getByRole('listitem')).toHaveCount(4);
    await expect(page.locator('.backdrop')).toHaveCount(0);
    await expect(page.getByRole('complementary', { name: 'You look at' })).toHaveCount(0);
  }
  await expect(page.getByRole('link', { name: 'Plan a route' })).toHaveAttribute('href', '#/planner/new');
  await page.getByRole('button', { name: 'Add a GPX or FIT file' }).click();
  await expect(page.getByRole('dialog', { name: 'Add files' })).toBeVisible();
});

test('the view of the day shows by the rule with its credits; a missing image reads the manifest again, then shows the plain ground', async ({ page }) => {
  // The first live set is gone, and the manifest then names a new set. Later a set is gone, and the manifest names it again.
  const sets = ['aaaaaaaaaaaaaaaa', 'bbbbbbbbbbbbbbbb'], gone = new Set(['aaaaaaaaaaaaaaaa']);
  const manifests = [];
  await start(page, (route, path) => {
    if (path === '/manifest.json') {
      const live = sets.length > 1 ? sets.shift() : sets[0];
      manifests.push(live);
      return route.fulfill({ json: manifest(live), headers: { 'access-control-allow-origin': '*' } });
    }
    return gone.has(path.slice(1, 17)) ? route.fulfill({ status: 404 }) : route.fulfill({ body: PIXEL, contentType: 'image/webp' });
  });
  await page.goto('/planner.html#/');

  // 13 Nov 2026 is day 20,770 since 1970: view 20,770 mod 3 = 1, at noon.
  const box = page.getByRole('complementary', { name: 'You look at' });
  await expect(box.getByRole('heading', { name: 'View b' })).toBeVisible();
  await expect(page.locator('img.backdrop')).toHaveAttribute('src', `${HOME_VIEWS}/bbbbbbbbbbbbbbbb/b/noon.webp`);
  await box.getByText('Data credits').click();
  await expect(box.getByText('© Data of bbbbbbbbbbbbbbbb')).toBeVisible();
  await expect(box.getByRole('button', { name: 'Next view' })).toHaveCount(0);
  expect(manifests).toEqual(['aaaaaaaaaaaaaaaa', 'bbbbbbbbbbbbbbbb']);

  sets[0] = 'cccccccccccccccc';
  gone.add('cccccccccccccccc');
  await page.goto('/planner.html#/routes');
  await page.goto('/planner.html#/');
  await expect.poll(() => manifests).toEqual(['aaaaaaaaaaaaaaaa', 'bbbbbbbbbbbbbbbb', 'cccccccccccccccc', 'cccccccccccccccc']);
  await expect(page.locator('.backdrop')).toHaveCount(0);
  await expect(box).toHaveCount(0);
});

test('the text, the cards, the search and each button open what they name', async ({ page }) => {
  await start(page);
  await fill(page, { routes: [lake], rides });
  await page.goto('/planner.html#/');

  await expect(page.getByText('This year, by bike')).toBeVisible();
  await expect(page.getByText('Your last ride was Kaiserstuhl, 4 days ago.')).toBeVisible();
  await expect(page.getByRole('link', { name: 'Plan a route', exact: true })).toHaveAttribute('href', '#/planner/new');
  await page.getByRole('link', { name: 'Look back at it' }).click();
  await expect(page).toHaveURL(/#\/ride\/20261109T090000Z$/);
  await expect(page.getByText('Kaiserstuhl').first()).toBeVisible();
  await page.goBack();

  // Routes come first; the switch shows only the chosen kind.
  const drawer = page.getByRole('region', { name: 'Your routes' });
  await expect(drawer.getByRole('link', { name: 'Lake test' })).toHaveAttribute('href', '#/route/lake');
  await expect(drawer.getByRole('link', { name: 'All 1 in Routes ›' })).toHaveAttribute('href', '#/routes');
  await drawer.getByRole('radio', { name: 'Rides' }).click();
  const ridden = page.getByRole('region', { name: 'Your rides' });
  await expect(ridden.getByRole('link')).toHaveText(['All 2 in Routes ›', 'Kaiserstuhl', 'Schauinsland']);
  await expect(ridden.getByRole('link', { name: 'Kaiserstuhl' })).toHaveAttribute('href', '#/ride/20261109T090000Z');
  await expect(ridden.getByRole('link', { name: 'All 2 in Routes ›' })).toHaveAttribute('href', '#/routes/ridden');

  // A name finds the route; "Show all" opens Routes with the text.
  const search = page.getByRole('searchbox', { name: 'Search', exact: true });
  await search.fill('lake');
  const results = page.getByRole('region', { name: 'Search results' });
  await expect(results.getByRole('link', { name: /Lake test/ })).toHaveAttribute('href', '#/route/lake');
  await results.getByRole('button', { name: 'Show all 1 route in Routes ›' }).click();
  await expect(page).toHaveURL(/#\/routes$/);
  await expect(page.getByRole('searchbox', { name: 'Search your routes' })).toHaveValue('lake');
  await page.goBack();

  // A place opens Routes with its chip, or the planner.
  await search.fill('Testdorf');
  await results.getByRole('button', { name: 'Yours near here (1)' }).click();
  await expect(page).toHaveURL(/#\/routes$/);
  await expect(page.getByRole('group').getByText('Testdorf')).toBeVisible();
  await expect(page.getByText('1 route', { exact: true })).toBeVisible();
  await page.goBack();
  await search.fill('Testdorf');
  await results.getByRole('button', { name: 'Plan a route here' }).click();
  await expect(page).toHaveURL(/#\/planner$/);
  const canvas = page.locator('.maplibregl-canvas');
  await expect(canvas).toBeVisible();
  // The map shows the box of the place: a start at the middle of the map is in the box.
  await page.waitForLoadState('networkidle');
  const map = await canvas.boundingBox();
  await page.mouse.click(map.x + map.width / 2, map.y + map.height / 2);
  await page.getByRole('button', { name: 'Start here' }).click();
  await expect(page.getByText('Choose your finish')).toBeVisible();
  await page.goto('/planner.html?cover=none');
  await page.waitForFunction(() => window.coverTest);
  const [lon, lat] = await page.evaluate(async () => (await window.coverTest.library.routes()).find(route => route.trip.points.length === 1).trip.points[0].coordinate);
  expect(lon).toBeGreaterThan(PLACE.west);
  expect(lon).toBeLessThan(PLACE.east);
  expect(lat).toBeGreaterThan(PLACE.south);
  expect(lat).toBeLessThan(PLACE.north);

  // The open plan comes before the year.
  await fill(page, { active: 'lake' });
  await page.goto('/planner.html#/');
  await expect(page.getByText('Continue where you stopped')).toBeVisible();
  await expect(page.getByRole('link', { name: 'Open in the planner' })).toHaveAttribute('href', '#/planner/lake');
  await expect(page.getByRole('link', { name: 'Plan a new route' })).toHaveAttribute('href', '#/planner/new');
});
