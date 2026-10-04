import { readFile } from 'node:fs/promises';
import { expect, test } from '@playwright/test';
import { CORS_PREFLIGHT, respond } from './stubs.js';

const trip = {
  live: true, mode: 'route', bike: 'touring', days: 3, budget: 'days', target: 3, limit: 50, variant: 'valley',
  points: [
    { id: 'start', kind: 'start', coordinate: [7.8, 48], label: 'Home', progress: 0 },
    { id: 'finish', kind: 'finish', coordinate: [7.9, 48.1], label: 'Camp', progress: 1 },
  ],
};

const planFile = name => ({ format: 'openbikecomputer-plan', version: 1, name, trip, versions: [] });

async function start(page) {
  await page.route(url => url.hostname !== '127.0.0.1', route => {
    if (route.request().method() === 'OPTIONS') return route.fulfill(CORS_PREFLIGHT);
    const response = respond(route.request());
    return response ? route.fulfill(response) : route.abort();
  });
  await page.goto('/planner.html');
  await expect(page.getByRole('button', { name: 'My plans', exact: true })).toBeEnabled();
}

async function importFile(page, file) {
  await page.getByLabel('Import plan or GPX files').setInputFiles({ name: 'tour.obcplan', mimeType: 'application/json', buffer: Buffer.from(JSON.stringify(file)) });
}

const library = page => page.getByRole('region', { name: 'My plans', exact: true });
const row = (page, name) => library(page).locator('li').filter({ has: page.locator('strong').filter({ hasText: new RegExp(`^${name.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}$`) }) });

test('plans survive new, switch and reload; checkpoints and files stay with their plan', async ({ page }, testInfo) => {
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await start(page);
  await page.getByRole('button', { name: 'My plans', exact: true }).click();
  await expect(library(page)).toContainText('Saved in this browser');
  await importFile(page, planFile('Weekend tour'));
  await expect(row(page, 'Weekend tour')).toBeVisible();
  await library(page).getByRole('button', { name: 'Close My plans' }).click();
  await page.getByRole('button', { name: 'Save version', exact: true }).click();
  await page.getByRole('textbox', { name: 'Version name' }).fill('Valley option');
  await page.getByRole('button', { name: 'Save checkpoint', exact: true }).click();
  await expect(page.locator('.versions li')).toContainText('Valley option');
  await page.getByRole('button', { name: 'Saved versions' }).click();
  await page.getByRole('button', { name: 'New route', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'New plan', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Undo', exact: true })).toBeDisabled();
  await page.getByRole('button', { name: 'My plans', exact: true }).click();
  await expect(library(page).locator('li')).toHaveCount(1);
  await importFile(page, planFile('Next ride'));
  await expect(library(page).locator('li')).toHaveCount(2);
  await row(page, 'Next ride').getByRole('button', { name: 'Rename', exact: true }).click();
  await page.getByRole('textbox', { name: 'Plan name' }).fill('Autumn ride');
  await library(page).locator('form').getByRole('button', { name: 'Rename', exact: true }).click();
  await expect(row(page, 'Autumn ride')).toBeVisible();
  await row(page, 'Weekend tour').getByRole('button', { name: /^Weekend tour/ }).click();
  await expect(page.getByRole('heading', { name: 'Weekend tour', exact: true })).toBeVisible();
  await page.reload();
  await expect(page.getByRole('heading', { name: 'Weekend tour', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Saved versions' }).click();
  await expect(page.locator('.versions li')).toContainText('Valley option');
  await page.getByRole('button', { name: 'Saved versions' }).click();
  await page.getByRole('button', { name: 'My plans', exact: true }).click();
  const downloadEvent = page.waitForEvent('download');
  await row(page, 'Weekend tour').getByRole('button', { name: 'Download', exact: true }).click();
  const download = await downloadEvent;
  const backup = JSON.parse(await readFile(await download.path(), 'utf8'));
  expect(backup).toMatchObject({ format: 'openbikecomputer-plan', name: 'Weekend tour', trip: { points: trip.points }, versions: [{ name: 'Valley option' }] });
  await row(page, 'Weekend tour').getByRole('button', { name: 'Duplicate', exact: true }).click();
  await expect(library(page).locator('li')).toHaveCount(3);
  await expect(page.getByRole('heading', { name: 'Weekend tour (copy)', exact: true })).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath('library-desktop.png') });
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(library(page)).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await page.screenshot({ path: testInfo.outputPath('library-phone.png') });
  page.once('dialog', dialog => dialog.accept());
  await row(page, 'Weekend tour (copy)').getByRole('button', { name: 'Delete', exact: true }).click();
  await expect(library(page).locator('li')).toHaveCount(2);
  await expect(page.getByRole('heading', { name: 'New plan', exact: true })).toBeVisible();
  await importFile(page, backup);
  await expect(library(page).locator('li')).toHaveCount(3);
  await library(page).getByRole('button', { name: 'Close My plans' }).click();
  await page.getByRole('button', { name: 'Saved versions' }).click();
  await expect(page.locator('.versions li')).toContainText('Valley option');
  expect(errors).toEqual([]);
});

test('GPX files open as a new trip with a transfer and leave the open plan in the library', async ({ page }) => {
  const gpx = (name, line) => ({ name, mimeType: 'application/gpx+xml', buffer: Buffer.from(`<gpx><trk><name>${name.replace('.gpx', '')} ride</name><trkseg>${
    line.map(([lon, lat]) => `<trkpt lat="${lat}" lon="${lon}"/>`).join('')}</trkseg></trk></gpx>`) });
  await start(page);
  await page.getByRole('button', { name: 'My plans', exact: true }).click();
  await importFile(page, planFile('Keep me'));
  await page.getByLabel('Import plan or GPX files').setInputFiles([
    gpx('north.gpx', [[7.8, 48], [7.82, 48.01], [7.85, 48]]), gpx('south.gpx', [[7.9, 47.95], [7.92, 47.96]])]);
  await expect(library(page).getByRole('region', { name: 'Import GPX' })).toContainText('New trip · 2 days');
  await page.getByRole('button', { name: "Keep the file's line", exact: true }).click();
  await expect(page.getByRole('heading', { name: 'north ride', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'My plans', exact: true }).click();
  await expect(library(page).locator('li')).toHaveCount(2);
  await expect(row(page, 'north ride')).toContainText('2 days · 1 night pinned');
  await expect(row(page, 'Keep me')).toBeVisible();
});

test('invalid imports and failed writes preserve the open plan and offer a download', async ({ page }) => {
  await start(page);
  await page.getByRole('button', { name: 'My plans', exact: true }).click();
  await importFile(page, planFile('Keep me'));
  await expect(row(page, 'Keep me')).toBeVisible();
  await importFile(page, { ...planFile('Broken'), trip: {} });
  await expect(library(page).getByRole('alert')).toContainText('invalid plan');
  await expect(library(page).locator('li')).toHaveCount(1);
  await expect(page.getByRole('heading', { name: 'Keep me', exact: true })).toBeVisible();
  await library(page).getByRole('button', { name: 'Close My plans' }).click();
  await page.evaluate(() => {
    const put = IDBObjectStore.prototype.put;
    globalThis.restorePlanWrites = () => { IDBObjectStore.prototype.put = put; };
    IDBObjectStore.prototype.put = function () { throw new DOMException('Full', 'QuotaExceededError'); };
  });
  await page.getByRole('combobox', { name: 'Plan type', exact: true }).click();
  await page.getByRole('option', { name: 'Multi-day trip', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Download plan', exact: true })).toBeVisible();
  await expect(page.locator('.status-line')).toContainText('storage is full');
  await page.getByRole('button', { name: 'New trip', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Keep me', exact: true })).toBeVisible();
  await page.evaluate(() => globalThis.restorePlanWrites());
  await page.getByRole('button', { name: 'Retry save', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Download plan', exact: true })).toHaveCount(0);
  await page.reload();
  await expect(page.getByRole('heading', { name: 'Keep me', exact: true })).toBeVisible();
  await expect(page.getByRole('combobox', { name: 'Plan type' })).toContainText('Multi-day trip');
});
