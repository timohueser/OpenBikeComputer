import { expect, test } from '@playwright/test';
import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { HOSTS, respond } from './stubs.js';

const REPO = join(dirname(fileURLToPath(import.meta.url)), '../../../../..');
const PLANNER_CATALOG = 'https://maps.openbikecomputer.com/planner/catalog.json';
const CORS = { 'access-control-allow-origin': '*' };
/** The map that `obcm-assemble` writes from the cells of the fixture catalog. */
const PINNED_MAP = join(REPO, 'builder/wasm/tests/fixture/expected/map.obcm');
const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');

// The fixture catalog of the builder journey: one region, "Bridge Fixture", of two detail cells side by side.
let server, origin;
test.beforeAll(async () => {
  server = spawn('python3', [join(REPO, 'tools/fixture_catalog.py'), '--catalog', 'web-assemble'], { stdio: ['ignore', 'pipe', 'inherit'] });
  origin = await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.stdout.once('data', chunk => resolve(String(chunk).split(' ')[0]));
  });
});
test.afterAll(() => server?.kill());

/** A route from the west cell of the region into its east cell. */
const summary = { line: [7.57, 47.3, 7.59, 47.3, 7.65, 47.3, 7.7, 47.3], heights: [400, 400, 400, 400], bounds: [7.57, 47.3, 7.7, 47.3], lengthM: 9800, climbM: 0, highestM: 400, seconds: 1800, bike: 'gravel', days: 1 };
/** A box in the west cell. */
const west = { minLat: 47_280_000, minLon: 7_560_000, maxLat: 47_320_000, maxLon: 7_595_000 };
const device = {
  serial: '9F3A00C1E2B44F2A', storeId: '0f5e9a7c2b3d4e1f8a6b5c4d3e2f1a0b', at: Date.UTC(2026, 10, 7, 12), map: 'Alps', mapObject: '7:1', built: null, routes: 0,
  bytes: { map: 9_000_000, routes: 0, rides: 0, other: 0 }, came: null, skipped: [],
};

/**
 * Answers the planner catalogue with the fixture catalog as the device catalog, and nothing else outside the machine.
 * `tiles: 'answer'` answers the tile host with the stubs, and `tiles: 'fail'` with 503. `outside` collects each other request
 * that leaves the machine.
 */
async function open(page, hash, { tiles, outside = [] } = {}) {
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.route(url => url.hostname !== '127.0.0.1', route => {
    const request = route.request();
    if (request.url() === PLANNER_CATALOG) {
      return route.fulfill({ headers: CORS, json: { format: 1, active: { id: 'test', device_catalog: `${origin}/catalog/catalog.json` } } });
    }
    outside.push(request.url());
    if (!tiles || new URL(request.url()).hostname !== HOSTS.tiles) return route.abort();
    return route.fulfill(tiles === 'fail' ? { headers: CORS, status: 503 } : respond(request) ?? { headers: CORS, status: 204 });
  });
  // The catalog has another port than the page, so its answers need the header that the real host sends.
  await page.route(url => url.origin === origin, async route => {
    const response = await route.fetch();
    await route.fulfill({ response, headers: { ...response.headers(), ...CORS } });
  });
  await page.goto(`/planner.html${hash}`);
  return errors;
}

/** Runs `work` with the library of the app, on the test page of a cover. */
async function library(page, work, argument) {
  await page.goto('/planner.html?cover=none');
  await page.waitForFunction(() => window.coverTest);
  return page.evaluate(work, argument);
}

const nextMap = page => library(page, () => window.coverTest.library.state('nextMap'));

test('with an empty library, a click on the map adds its region, and Done keeps the map for the device', async ({ page }) => {
  const errors = await open(page, '#/device/area');
  await expect(page.getByRole('heading', { name: 'The area of the map', level: 1 })).toBeVisible();
  await expect(page.getByRole('banner').getByRole('link', { name: 'Device' })).toHaveAttribute('aria-current', 'page');
  await expect(page.getByText('Click the map to add a region.')).toBeVisible();
  await expect(page.locator('.parts li')).toHaveCount(0);
  await expect(page.getByText('leaves this map')).toHaveCount(0);

  await page.locator('.leaflet-container').click();
  await expect(page.locator('.parts li')).toHaveText(['Bridge Fixture']);
  await expect(page.getByText(/^[\d.]+ KB to download$/)).toBeVisible();
  await page.getByRole('button', { name: 'Done' }).click();
  await expect(page).toHaveURL(/#\/device$/);
  expect(errors).toEqual([]);
  expect(await nextMap(page)).toMatchObject({ regions: ['bridge-fixture'], boxes: [], light: 'default', dark: 'dusk' });
});

test('a route that waits and leaves the area gives the warning, the box around it ends it, and the Device page shows the new area', async ({ page }) => {
  await library(page, async ({ summary, west, device }) => {
    const { library } = window.coverTest;
    await library.saveRoute({ id: 'pass', name: 'Pass day', updatedAt: 0, trip: {}, summary });
    await library.setState('waiting', ['pass']);
    await library.setState('nextMap', { regions: [], boxes: [west], light: 'default', dark: 'dusk', bytes: 1 });
    localStorage.setItem('obc-device', JSON.stringify(device));
  }, { summary, west, device });
  const errors = await open(page, '#/device/area');
  await expect(page.locator('.parts li')).toHaveCount(1);
  await expect(page.getByText('On the device: Alps · 8.6 MB')).toBeVisible();
  await expect(page.getByRole('heading', { name: '1 route leaves this map' })).toBeVisible();
  await expect(page.getByText(/^“Pass day” has [\d.]+\skm in Bridge Fixture\.$/)).toBeVisible();
  await expect(page.getByRole('button', { name: 'All of Bridge Fixture' })).toBeVisible();

  await page.getByRole('button', { name: 'Add the box around it' }).click();
  await expect(page.getByText('leaves this map')).toHaveCount(0);
  await expect(page.locator('.parts li')).toHaveCount(2);
  await page.getByRole('button', { name: 'Done' }).click();

  const row = page.locator('.row', { has: page.getByText('Map', { exact: true }) });
  await expect(row).toContainText(/New area · [\d.]+ KB/);
  await expect(row.getByRole('link', { name: 'Change the area of the map' })).toHaveAttribute('href', '#/device/area');
  await expect(row.getByRole('button')).toHaveCount(0);
  expect(errors).toEqual([]);
  expect((await nextMap(page)).boxes).toHaveLength(2);
});

test('a box past the map data gives the note, and the credits of the data and of the base map are behind one button', async ({ page }) => {
  await library(page, west => window.coverTest.library.setState('nextMap', { regions: [], boxes: [{ ...west, maxLon: 7_900_000 }], light: 'default', dark: 'dusk', bytes: 1 }), west);
  const errors = await open(page, '#/device/area');
  await expect(page.getByText('Map data is missing in a part of this area.')).toBeVisible();
  const baseCredit = page.locator('.leaflet-control-attribution'), dataCredits = page.locator('.data-credits');
  await expect(baseCredit).toBeHidden();
  await expect(dataCredits).toBeHidden();
  await page.getByRole('button', { name: 'Toggle attribution' }).click();
  await expect(baseCredit).toBeVisible();
  await expect(dataCredits).toContainText('Synthetic geometry');
  await expect(dataCredits.getByRole('link', { name: 'CC0-1.0' })).toHaveAttribute('href', 'https://creativecommons.org/publicdomain/zero/1.0/');
  // The region of the fixture lies under the credits at this zoom, and a click on them adds no region.
  const map = await page.locator('.leaflet-container').boundingBox();
  await page.mouse.move(map.x + map.width / 2, map.y + map.height / 2);
  for (let i = 0; i < 3; i++) await page.mouse.wheel(0, -300);
  await dataCredits.click({ position: { x: 20, y: 10 } });
  await expect(page.locator('.parts li')).toHaveCount(1);
  await page.getByRole('button', { name: 'Toggle attribution' }).click();
  await expect(baseCredit).toBeHidden();
  await expect(dataCredits).toBeHidden();

  await page.getByRole('button', { name: 'Remove Box 1' }).click();
  await page.locator('.leaflet-container').click();
  await expect(page.getByText(/to download$/)).toBeVisible();
  await expect(page.getByText('is missing')).toHaveCount(0);
  expect(errors).toEqual([]);
});

test('the window Colours opens the colour editor over it, and Cancel goes back to the window', async ({ page }) => {
  const errors = await open(page, '#/device/area');
  await page.getByRole('button', { name: 'Change' }).click();
  const colours = page.getByRole('dialog', { name: 'Colours' });
  await colours.getByRole('button', { name: 'Edit the colours' }).click();
  const editor = page.getByRole('dialog', { name: 'The colours of the day' });
  await expect(editor.getByRole('textbox', { name: 'Name' })).toHaveValue('My Bridge Fixture');
  await editor.getByRole('button', { name: 'Cancel' }).click();
  await expect(editor).toHaveCount(0);
  await expect(colours.getByRole('button', { name: 'Bridge Fixture' })).toHaveAttribute('aria-pressed', 'true');
  expect(errors).toEqual([]);
});

test('the base map loads in the light and the dark look, from the tile host only', async ({ page }) => {
  const outside = [];
  const light = page.waitForResponse(`https://${HOSTS.tiles}/sprites/light.json`);
  const errors = await open(page, '#/device/area', { tiles: 'answer', outside });
  await light;
  await expect(page.locator('.leaflet-gl-layer canvas')).toBeVisible();
  const dark = page.waitForResponse(`https://${HOSTS.tiles}/sprites/dark.json`);
  await page.evaluate(() => { document.documentElement.dataset.theme = 'dark'; });
  await dark;
  await expect(page.locator('.basemap-warning')).toHaveCount(0);
  expect(outside.length).toBeGreaterThan(0);
  expect(outside.every(url => new URL(url).hostname === HOSTS.tiles)).toBe(true);
  expect(errors).toEqual([]);
});

test('when the base map does not load, the map says so and a click still adds a region', async ({ page }) => {
  const outside = [];
  const errors = await open(page, '#/device/area', { tiles: 'fail', outside });
  await expect(page.getByRole('status').filter({ hasText: 'Map background unavailable.' })).toBeVisible();
  await page.locator('.leaflet-container').click();
  await expect(page.locator('.parts li')).toHaveText(['Bridge Fixture']);
  expect(outside.every(url => new URL(url).hostname === HOSTS.tiles)).toBe(true);
  expect(errors).toEqual([]);
});

test('a browser with no WebGL2 gets the note and asks the tile host for nothing', async ({ page }) => {
  await page.addInitScript(() => {
    const original = HTMLCanvasElement.prototype.getContext;
    HTMLCanvasElement.prototype.getContext = function (type, ...args) {
      return type === 'webgl2' ? null : original.call(this, type, ...args);
    };
  });
  const outside = [];
  const errors = await open(page, '#/device/area', { tiles: 'answer', outside });
  await expect(page.locator('.basemap-warning')).toHaveText('Map background unavailable.');
  await expect(page.locator('.leaflet-gl-layer')).toHaveCount(0);
  expect(outside).toEqual([]);
  expect(errors).toEqual([]);
});

test('"Send the map" builds the map that waits and puts it on the device; Stop in the middle keeps it waiting', async ({ page }) => {
  await library(page, () => window.coverTest.library.setState('nextMap', { regions: ['bridge-fixture'], boxes: [], light: 'default', dark: 'dusk', bytes: 16_000 }));
  const errors = await open(page, '?fake-device#/device');
  await page.getByRole('button', { name: 'Connect a device' }).click();
  const row = page.locator('.row', { has: page.getByText('Map', { exact: true }) });
  await expect(row).toContainText('New area · 15.6 KB');
  await expect(page.locator('.row', { hasText: 'In sync.' })).toBeVisible();

  // The device holds its answer to the list before the put, so the send stays after the build.
  await page.evaluate(() => window.deviceTest.hold());
  await row.getByRole('button', { name: 'Send the map' }).click();
  await expect(row).toContainText('Build');
  await expect(row.getByRole('progressbar')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Disconnect' })).toBeDisabled();
  await expect(page.getByRole('button', { name: 'Sync now' })).toBeDisabled();
  await row.getByRole('button', { name: 'Stop' }).click();
  await page.evaluate(() => window.deviceTest.release());
  await expect(row).toContainText('The new map was not sent. “Send the map” starts again.');
  await expect(row).toContainText('New area · 15.6 KB');
  await expect(page.locator('.row', { has: page.getByText('Memory card', { exact: true }) })).toContainText('Map 39.1 KB');

  await row.getByRole('button', { name: 'Send the map' }).click();
  await expect(row).toContainText('Bridge Fixture');
  await expect(row).not.toContainText('New area');
  await expect(row.getByRole('button')).toHaveCount(0);
  await expect(row.getByRole('link', { name: 'Change the area of the map' })).toBeVisible();
  // The card holds the map that `obcm-assemble` wrote from the same cells, byte for byte.
  const sent = Buffer.from(await page.evaluate(() => window.deviceTest.map()));
  expect(sha256(sent)).toBe(sha256(await readFile(PINNED_MAP)));
  expect(errors).toEqual([]);
  expect(await nextMap(page)).toBeNull();
});

test('Stop while the map goes to the device keeps the map of the card, and the new map waits', async ({ page }) => {
  await library(page, () => window.coverTest.library.setState('nextMap', { regions: ['bridge-fixture'], boxes: [], light: 'default', dark: 'dusk', bytes: 16_000 }));
  const errors = await open(page, '?fake-device#/device');
  await page.getByRole('button', { name: 'Connect a device' }).click();
  const row = page.locator('.row', { has: page.getByText('Map', { exact: true }) });
  await expect(page.locator('.row', { hasText: 'In sync.' })).toBeVisible();
  const before = await page.evaluate(() => window.deviceTest.map());

  await page.evaluate(() => window.deviceTest.holdPut());
  await row.getByRole('button', { name: 'Send the map' }).click();
  await expect(row).toContainText(/Send \d+ %/);
  await row.getByRole('button', { name: 'Stop' }).click();
  await page.evaluate(() => window.deviceTest.release());
  await expect(row).toContainText('The new map was not sent. “Send the map” starts again.');
  await expect(row).toContainText('New area · 15.6 KB');
  expect(await page.evaluate(() => window.deviceTest.map())).toEqual(before);
  expect(errors).toEqual([]);
});
