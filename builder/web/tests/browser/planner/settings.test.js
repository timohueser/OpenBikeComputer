import { expect, test } from '@playwright/test';
import { CORS_PREFLIGHT, respond } from './stubs.js';

const summary = { line: [8, 47, 8.01, 47], heights: [400, 420], bounds: [8, 47, 8.01, 47], lengthM: 760, climbM: 20, highestM: 420, seconds: 300, bike: 'gravel' };

async function open(page) {
  await page.goto('/planner.html#/settings');
  await expect(page.getByRole('dialog', { name: 'Settings' })).toBeVisible();
}

function stub(page) {
  return page.route(url => url.hostname !== '127.0.0.1', route => {
    if (route.request().method() === 'OPTIONS') return route.fulfill(CORS_PREFLIGHT);
    const response = respond(route.request());
    return response ? route.fulfill(response) : route.abort();
  });
}

const drawn = (id, kind, lon, from, to, extra = {}) => ({
  id, kind, label: id, coordinate: [lon, 47], ...extra,
  ...kind !== 'start' && { leg: 'drawn', drawn: [[lon - 0.05, 47, from], [lon - 0.025, 47, (from + to) / 2], [lon, 47, to]] },
});
// A route of two days with drawn legs, so its line needs no routing request.
const twoDays = {
  id: 'two', revision: 0, name: 'Two days', updatedAt: 0,
  trip: {
    mode: 'trip', bike: 'touring', days: 2, daysSet: true, restAfter: [], routeOrder: ['night-1'], points: [
      drawn('start', 'start', 8, 400, 400), drawn('night-1', 'night', 8.1, 400, 600, { night: 1, label: 'Inn' }), drawn('finish', 'finish', 8.2, 600, 400),
    ],
  },
  summary: { line: [8, 47, 8.1, 47, 8.2, 47], heights: [400, 600, 400], bounds: [8, 47, 8.2, 47], lengthM: 15_184, climbM: 200, highestM: 600, seconds: 4000, bike: 'touring', days: 2 },
};

test('the settings are a window over the page, and the page under it stays as it was', async ({ page }) => {
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await stub(page);
  const settings = page.getByRole('dialog', { name: 'Settings' });
  const planner = page.getByRole('navigation', { name: 'Pages' }).getByRole('link', { name: 'Planner' });

  // A direct visit shows the window over the first page; Close leaves that page.
  await open(page);
  await expect(planner).toHaveAttribute('aria-current', 'page');
  await settings.getByRole('button', { name: 'Close' }).click();
  await expect(settings).toHaveCount(0);
  await expect(page).toHaveURL(/#\/$/);
  await expect(planner).toHaveAttribute('aria-current', 'page');

  await page.goto('/planner.html?cover=none');
  await page.waitForFunction(() => window.coverTest);
  await page.evaluate(record => window.coverTest.library.saveRoute(record), twoDays);
  await page.goto('/planner.html#/route/two');
  const day2 = page.getByRole('radio', { name: 'Day 2' });
  await day2.click();
  await expect(day2).toBeChecked();

  // Close, then Back: each time the Route page keeps its chosen day.
  for (const close of [() => settings.getByRole('button', { name: 'Close' }).click(), () => page.goBack()]) {
    await page.getByRole('link', { name: 'Settings' }).click();
    await expect(settings).toBeVisible();
    await expect(page).toHaveURL(/#\/settings$/);
    await close();
    await expect(settings).toHaveCount(0);
    await expect(page).toHaveURL(/#\/route\/two$/);
    await expect(day2).toBeChecked();
  }
  expect(errors).toEqual([]);
});

const counts = page => page.getByRole('region', { name: 'Your library' }).locator('.count');

test('the settings set the look and the units, and a backup brings the library back after Delete all', async ({ page }) => {
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await stub(page);
  await open(page);
  await expect(counts(page)).toHaveText('0 routes and 0 rides are saved in this browser, on this computer only.');
  await expect(page.getByRole('button', { name: 'Save a backup file' })).toBeDisabled();
  await page.getByRole('radio', { name: 'Dark' }).click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  await expect(page.getByText('Set a value to see your zones.')).toHaveCount(2);
  await page.getByLabel('Maximum heart rate', { exact: true }).fill('185');
  await page.getByLabel('Threshold power (FTP)', { exact: true }).fill('250');
  await page.getByLabel('Threshold power (FTP)', { exact: true }).press('Tab');
  await expect(page.getByRole('list', { name: 'Maximum heart rate zones' }).getByRole('listitem'))
    .toHaveText(['Z1 ≤ 110', 'Z2 111–129', 'Z3 130–147', 'Z4 148–166', 'Z5 ≥ 167']);
  await expect(page.getByRole('list', { name: 'Threshold power (FTP) zones' }).getByRole('listitem'))
    .toHaveText(['Z1 ≤ 137', 'Z2 138–187', 'Z3 188–225', 'Z4 226–262', 'Z5 ≥ 263']);
  await expect(page.getByText('Set a value to see your zones.')).toHaveCount(0);

  // The test page of a cover gives the library of the app.
  await page.goto('/planner.html?cover=none');
  await page.waitForFunction(() => window.coverTest);
  await page.evaluate(async summary => {
    const { library } = window.coverTest;
    await library.saveRoute({ id: 'route', revision: 0, name: 'Route', updatedAt: 0, trip: {}, summary: { ...summary, days: 1 } });
    await library.addRide({ key: '20260801T070000Z', name: 'Ride', source: 'file', format: 'gpx', maxHr: null, ftp: null,
      summary: { ...summary, start: 1_785_481_200, movingS: 280 } }, new Blob(['ride']));
  }, summary);
  await open(page);
  await expect(counts(page)).toContainText('1 route and 1 ride');

  const saved = page.waitForEvent('download');
  await page.getByRole('button', { name: 'Save a backup file' }).click();
  const backup = await saved;
  expect(backup.suggestedFilename()).toMatch(/^openbikecomputer-\d{4}-\d\d-\d\d\.zip$/);

  await page.getByRole('radio', { name: 'Miles' }).click();
  await page.getByRole('button', { name: 'Delete all' }).click();
  const confirm = page.getByRole('dialog', { name: 'Delete all?' });
  await expect(confirm).toContainText('Close other tabs of this app first.');
  await Promise.all([page.waitForEvent('load'), confirm.getByRole('button', { name: 'Delete all' }).click()]);
  await expect(counts(page)).toContainText('0 routes and 0 rides');
  await expect(page.getByRole('radio', { name: 'Miles' })).toHaveAttribute('aria-checked', 'true');

  await page.locator('input[type="file"]').setInputFiles(await backup.path());
  await expect(page.getByRole('dialog', { name: 'Settings' }).getByRole('status')).toHaveText('Added 1 route and 1 ride.');
  await expect(counts(page)).toContainText('1 route and 1 ride');
  await expect(page.getByRole('radio', { name: 'Kilometres' })).toHaveAttribute('aria-checked', 'true');
  expect(errors).toEqual([]);
});
