import { expect, test } from '@playwright/test';

const summary = { line: [8, 47, 8.01, 47], heights: [400, 420], bounds: [8, 47, 8.01, 47], lengthM: 760, climbM: 20, highestM: 420, seconds: 300, bike: 'gravel', days: 1 };
const device = {
  serial: '9F3A00C1E2B44F2A', storeId: '0f5e9a7c2b3d4e1f8a6b5c4d3e2f1a0b', at: Date.UTC(2026, 10, 7, 12), map: 'Alps', routes: 6,
  bytes: { map: 9_000_000, routes: 20_000, rides: 2_000_000, other: 0 }, came: { day: '2026-11-07', count: 3 }, skipped: [],
};

/** Opens the Device page and fails on any request that leaves the machine, so the drawing never loads. */
async function open(page) {
  const errors = [], requests = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('request', request => requests.push(request.url()));
  await page.route(url => url.hostname !== '127.0.0.1', route => route.abort());
  await page.goto('/planner.html#/device');
  return { errors, requests };
}

const chip = page => page.getByRole('banner').getByRole('link', { name: 'OBC-4F2A' });

test('with no device remembered, the page offers the first connection and the note for a rider with no device', async ({ page }) => {
  const { errors } = await open(page);
  await expect(page.getByRole('heading', { name: 'Connect your OpenBikeComputer', level: 1 })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Connect a device' })).toBeEnabled();
  await expect(page.getByText('All of this product works without one.')).toBeVisible();
  await expect(page.getByRole('banner').getByRole('link', { name: 'Device' })).toHaveAttribute('aria-current', 'page');
  await expect(chip(page)).toHaveCount(0);
  expect(errors).toEqual([]);
});

/** Fills the library through the test page of a cover, which gives the library of the app. */
async function fill(page, state, remembered = null) {
  await page.goto('/planner.html?cover=none');
  await page.waitForFunction(() => window.coverTest);
  await page.evaluate(async ({ summary, state, remembered }) => {
    const { library } = window.coverTest;
    for (const [id, name] of [['lake', 'Lake loop'], ['pass', 'Pass day']]) await library.saveRoute({ id, revision: 0, name, updatedAt: 0, trip: {}, summary });
    for (const [key, value] of Object.entries(state)) await library.setState(key, value);
    if (remembered) localStorage.setItem('obc-device', JSON.stringify(remembered));
  }, { summary, state, remembered });
}

const row = (page, label) => page.locator('.row', { has: page.getByText(label, { exact: true }) });

test('a remembered device that is not connected shows the last sync and what waits', async ({ page }) => {
  await fill(page, { waiting: ['lake', 'pass'], leaving: ['old'] }, device);
  const { errors, requests } = await open(page);
  await expect(page.getByRole('heading', { name: 'OBC-4F2A', level: 1 })).toBeVisible();
  await expect(page.getByText('Not connected', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Connect', exact: true })).toBeEnabled();
  await expect(row(page, 'Sync')).toContainText('Last sync on 7 November');
  await expect(row(page, 'Waits for the device')).toContainText('2 routes: Lake loop · Pass day');
  await expect(row(page, 'Waits for the device').getByRole('link', { name: 'Lake loop' })).toHaveAttribute('href', '#/route/lake');
  await expect(page.getByText('1 route to remove from the device')).toBeVisible();
  await expect(page.getByText('Routes go to the device when you connect it.')).toBeVisible();
  await expect(chip(page)).toHaveAttribute('href', '#/device');
  expect(requests.filter(url => /catalog|\.obcm/.test(url))).toEqual([]);
  expect(errors).toEqual([]);
});

test('connected, the rows show the sync, the routes, the rides, the memory card, the map and the software', async ({ page }) => {
  // The pretend device holds "Lake loop", which the library sent to its card.
  await fill(page, { sent: { lake: { device: `${device.storeId}:9`, line: '' } } });
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.route(url => url.hostname !== '127.0.0.1', route => route.abort());
  await page.goto('/planner.html?fake-device#/device');
  await page.getByRole('button', { name: 'Connect a device' }).click();
  await expect(page.getByText('Connected', { exact: true })).toBeVisible();
  await expect(row(page, 'Sync')).toContainText(/In sync\. Last sync today at \d\d:\d\d\./);
  await expect(row(page, 'Sync')).toContainText('2 rides came in');
  await expect(row(page, 'Sync').getByRole('button', { name: 'Sync now' })).toBeEnabled();
  await expect(row(page, 'Routes')).toContainText('1 on the device');
  await expect(row(page, 'Routes').getByRole('link', { name: 'Lake loop' })).toHaveAttribute('href', '#/route/lake');
  await expect(row(page, 'Rides')).toContainText('2 in your library came from this device');
  await expect(row(page, 'Rides')).toContainText('The last on');
  await expect(row(page, 'Memory card')).toContainText('Map 39.1 KB');
  await expect(row(page, 'Memory card')).toContainText('Routes 1.2 KB');
  await expect(row(page, 'Map')).toContainText('Baden-Württemberg');
  await expect(row(page, 'Software')).toContainText('Version 0.9.2');
  await expect(row(page, 'Software').getByRole('button')).toHaveCount(0);
  await page.evaluate(() => window.deviceTest.offer('0.9.3'));
  await expect(row(page, 'Software')).toContainText('Version 0.9.3 is ready');
  await expect(row(page, 'Software')).toContainText('The device has version 0.9.2');
  await expect(row(page, 'Software').getByRole('button', { name: 'Update' })).toBeEnabled();
  await expect(chip(page).locator('.dot')).toHaveClass(/connected/);
  expect(errors).toEqual([]);
});

test('a sync turns the icon beside the chip, and after a problem the icon starts the sync again', async ({ page }) => {
  // A waiting id with no route is a problem of the sync.
  await fill(page, { waiting: ['missing'] });
  await page.route(url => url.hostname !== '127.0.0.1', route => route.abort());
  await page.goto('/planner.html?fake-device#/device');
  await page.waitForFunction(() => window.deviceTest);
  await page.evaluate(() => window.deviceTest.hold());
  await page.getByRole('button', { name: 'Connect a device' }).click();
  const banner = page.getByRole('banner');
  await expect(banner.getByRole('img', { name: 'Syncing' })).toBeVisible();
  await expect(row(page, 'Sync')).toContainText('Syncing');
  await expect(row(page, 'Sync').getByRole('button', { name: 'Sync now' })).toBeDisabled();
  await page.evaluate(() => window.deviceTest.release());
  await expect(row(page, 'Sync')).toContainText('A route that waits cannot be read.');
  await expect(row(page, 'Sync').getByRole('button', { name: 'Try again' })).toBeEnabled();
  await expect(banner.getByRole('img', { name: 'Syncing' })).toHaveCount(0);
  await page.evaluate(() => window.deviceTest.hold());
  await banner.getByRole('button', { name: 'Sync again' }).click();
  await expect(banner.getByRole('img', { name: 'Syncing' })).toBeVisible();
  await page.evaluate(() => window.deviceTest.release());
  await expect(banner.getByRole('button', { name: 'Sync again' })).toBeVisible();
});

test('the remembered device with a card that has no format is not connected, and the page names the map builder', async ({ page }) => {
  await fill(page, {}, device);
  await page.route(url => url.hostname !== '127.0.0.1', route => route.abort());
  await page.goto('/planner.html?fake-device#/device');
  await page.waitForFunction(() => window.deviceTest);
  await page.evaluate(() => window.deviceTest.plug(true));
  await expect(page.getByText('The memory card of OBC-4F2A needs a format.')).toBeVisible();
  await expect(page.getByRole('link', { name: 'The map builder' })).toBeVisible();
  await expect(page.getByText('Not connected', { exact: true })).toBeVisible();
  await expect(chip(page).locator('.dot')).not.toHaveClass(/connected/);
});

test('a browser with no WebUSB says that the cable works in Chrome and Edge, and offers no button', async ({ page }) => {
  await page.addInitScript(() => { delete Navigator.prototype.usb; });
  const { errors } = await open(page);
  await expect(page.getByRole('heading', { name: 'This browser cannot connect to the device', level: 1 })).toBeVisible();
  await expect(page.getByText('The cable connection works in Chrome and in Edge.', { exact: false })).toBeVisible();
  await expect(page.getByRole('main').getByRole('button')).toHaveCount(0);
  expect(errors).toEqual([]);
});
