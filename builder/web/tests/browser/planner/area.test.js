import { expect, test } from '@playwright/test';
import { spawn } from 'node:child_process';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const REPO = join(dirname(fileURLToPath(import.meta.url)), '../../../../..');
const PLANNER_CATALOG = 'https://maps.openbikecomputer.com/planner/catalog.json';
const CORS = { 'access-control-allow-origin': '*' };

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

/** Answers the planner catalogue with the fixture catalog as the device catalog, and nothing else outside the machine. */
async function open(page, hash) {
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.route(url => url.hostname !== '127.0.0.1', route => route.request().url() === PLANNER_CATALOG
    ? route.fulfill({ headers: CORS, json: { format: 1, active: { id: 'test', device_catalog: `${origin}/catalog/catalog.json` } } })
    : route.abort());
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
    await library.saveRoute({ id: 'pass', revision: 0, name: 'Pass day', updatedAt: 0, trip: {}, summary });
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
  await expect(row).toContainText('The new map was not sent, and its download is lost.');
  await expect(row).toContainText('New area · 15.6 KB');
  await expect(page.locator('.row', { has: page.getByText('Memory card', { exact: true }) })).toContainText('Map 39.1 KB');

  await row.getByRole('button', { name: 'Send the map' }).click();
  await expect(row).toContainText('Bridge Fixture');
  await expect(row).not.toContainText('New area');
  await expect(row.getByRole('button')).toHaveCount(0);
  await expect(row.getByRole('link', { name: 'Change the area of the map' })).toBeVisible();
  expect(errors).toEqual([]);
  expect(await nextMap(page)).toBeNull();
});
