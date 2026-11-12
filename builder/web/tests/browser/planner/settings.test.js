import { expect, test } from '@playwright/test';
import { CORS_PREFLIGHT, respond } from './stubs.js';

const summary = { line: [8, 47, 8.01, 47], heights: [400, 420], bounds: [8, 47, 8.01, 47], lengthM: 760, climbM: 20, highestM: 420, seconds: 300, bike: 'gravel' };

async function open(page) {
  await page.goto('/planner.html#/settings');
  await expect(page.getByRole('heading', { name: 'Settings', level: 1 })).toBeVisible();
}

const counts = page => page.getByRole('region', { name: 'Your library' }).locator('.count');

test('the settings set the look and the units, and a backup brings the library back after Delete all', async ({ page }) => {
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.route(url => url.hostname !== '127.0.0.1', route => {
    if (route.request().method() === 'OPTIONS') return route.fulfill(CORS_PREFLIGHT);
    const response = respond(route.request());
    return response ? route.fulfill(response) : route.abort();
  });
  await open(page);
  await expect(counts(page)).toHaveText('0 routes and 0 rides are saved in this browser, on this computer only.');
  await expect(page.getByRole('button', { name: 'Save a backup file' })).toBeDisabled();
  await page.getByRole('radio', { name: 'Dark' }).click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');

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
  await confirm.getByRole('button', { name: 'Delete all' }).click();
  await expect(counts(page)).toContainText('0 routes and 0 rides');
  await expect(page.getByRole('radio', { name: 'Miles' })).toHaveAttribute('aria-checked', 'true');

  await page.locator('input[type="file"]').setInputFiles(await backup.path());
  await expect(page.getByRole('status')).toHaveText('Added 1 route and 1 ride.');
  await expect(counts(page)).toContainText('1 route and 1 ride');
  await expect(page.getByRole('radio', { name: 'Kilometres' })).toHaveAttribute('aria-checked', 'true');
  expect(errors).toEqual([]);
});
