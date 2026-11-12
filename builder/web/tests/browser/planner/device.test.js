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

test('a remembered device that is not connected shows what waits and what was on it', async ({ page }) => {
  // The test page of a cover gives the library of the app.
  await page.goto('/planner.html?cover=none');
  await page.waitForFunction(() => window.coverTest);
  await page.evaluate(async ({ summary, device }) => {
    const { library } = window.coverTest;
    for (const [id, name] of [['lake', 'Lake loop'], ['pass', 'Pass day']]) await library.saveRoute({ id, revision: 0, name, updatedAt: 0, trip: {}, summary });
    await library.setState('waiting', ['lake', 'pass']);
    await library.setState('leaving', ['old']);
    localStorage.setItem('obc-device', JSON.stringify(device));
  }, { summary, device });

  const { errors, requests } = await open(page);
  await expect(page.getByRole('heading', { name: 'Your device is not connected', level: 1 })).toBeVisible();
  await expect(page.getByText('Last update on 7 November.')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Connect it again' })).toBeEnabled();
  await expect(page.getByText('2 routes: “Lake loop”, “Pass day”')).toBeVisible();
  await expect(page.getByText('1 route to remove from the device')).toBeVisible();
  await expect(page.getByText('Map: Alps')).toBeVisible();
  await expect(page.getByText('6 routes', { exact: true })).toBeVisible();
  await expect(chip(page)).toHaveAttribute('href', '#/device');
  expect(requests.filter(url => /catalog|\.obcm/.test(url))).toEqual([]);
  expect(errors).toEqual([]);
});

test('a browser with no WebUSB says that the cable works in Chrome and Edge, and offers no button', async ({ page }) => {
  await page.addInitScript(() => { delete Navigator.prototype.usb; });
  const { errors } = await open(page);
  await expect(page.getByRole('heading', { name: 'This browser cannot connect to the device', level: 1 })).toBeVisible();
  await expect(page.getByText('The cable connection works in Chrome and in Edge.', { exact: false })).toBeVisible();
  await expect(page.getByRole('main').getByRole('button')).toHaveCount(0);
  expect(errors).toEqual([]);
});
