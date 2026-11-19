import { readFile } from 'node:fs/promises';
import { expect, test } from '@playwright/test';
import { CORS_PREFLIGHT, respond } from './stubs.js';

const trip = {
  mode: 'route', bike: 'touring', days: 3, budget: 'days', target: 3, limit: 50, routeOrder: [],
  points: [
    { id: 'start', kind: 'start', coordinate: [7.8, 48], label: 'Home' },
    { id: 'finish', kind: 'finish', coordinate: [7.9, 48.1], label: 'Camp' },
  ],
};

const planFile = name => ({ format: 'openbikecomputer-plan', version: 1, name, trip });

async function start(page) {
  await page.route(url => url.hostname !== '127.0.0.1', route => {
    if (route.request().method() === 'OPTIONS') return route.fulfill(CORS_PREFLIGHT);
    const response = respond(route.request());
    return response ? route.fulfill(response) : route.abort();
  });
  await page.goto('/planner.html#/planner');
  await expect(page.getByRole('button', { name: 'My plans', exact: true })).toBeEnabled();
}

async function importFile(page, file) {
  await page.getByLabel('Import plan or GPX files').setInputFiles({ name: 'tour.obcplan', mimeType: 'application/json', buffer: Buffer.from(JSON.stringify(file)) });
}

/** The records of a store of the library database. */
const records = (page, store) => page.evaluate(store => new Promise((resolve, reject) => {
  const request = indexedDB.open('obc-library');
  request.onerror = () => reject(request.error);
  request.onsuccess = () => {
    const db = request.result, all = db.transaction(store).objectStore(store).getAll();
    all.onsuccess = () => { db.close(); resolve(all.result); };
    all.onerror = () => reject(all.error);
  };
}), store);

const saveButton = page => page.locator('.trip-bar .save > button');
const dialog = page => page.getByRole('dialog');
/** A change of the open route that needs no map: its plan type. */
async function changeType(page, type) {
  await page.getByRole('combobox', { name: 'Plan type', exact: true }).click();
  await page.getByRole('option', { name: type, exact: true }).click();
}
async function more(page, item) {
  await page.getByRole('button', { name: 'More ▾', exact: true }).click();
  await page.getByRole('button', { name: item, exact: true }).click();
}

const library = page => page.getByRole('region', { name: 'My plans', exact: true });
const row = (page, name) => library(page).locator('li').filter({ has: page.locator('strong').filter({ hasText: new RegExp(`^${name.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}$`) }) });

test('plans survive new, switch and reload, and a downloaded file opens again', async ({ page }, testInfo) => {
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await start(page);
  await page.getByRole('button', { name: 'My plans', exact: true }).click();
  await expect(library(page)).toContainText('Saved in this browser');
  await importFile(page, planFile('Weekend tour'));
  await expect(row(page, 'Weekend tour')).toBeVisible();
  await library(page).getByRole('button', { name: 'Close My plans' }).click();
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
  await page.getByRole('button', { name: 'My plans', exact: true }).click();
  const downloadEvent = page.waitForEvent('download');
  await row(page, 'Weekend tour').getByRole('button', { name: 'Download', exact: true }).click();
  const download = await downloadEvent;
  const backup = JSON.parse(await readFile(await download.path(), 'utf8'));
  expect(backup).toMatchObject({ format: 'openbikecomputer-plan', name: 'Weekend tour', trip: { points: trip.points } });
  expect(backup).not.toHaveProperty('versions');
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
  await expect(page.getByRole('heading', { name: 'Weekend tour', exact: true })).toBeVisible();
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
  await expect(row(page, 'north ride')).toContainText('2 days · 6.2 km');
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
  await changeType(page, 'Multi-day trip');
  await expect(page.getByRole('button', { name: 'Download plan', exact: true })).toBeVisible();
  await expect(page.locator('.status-line')).toContainText('Browser storage is full. Delete routes you do not need, then save again.');
  await saveButton(page).click();
  await expect(page.getByRole('button', { name: 'Download plan', exact: true })).toBeVisible();
  await expect(saveButton(page)).toHaveText('Save');
  await page.evaluate(() => globalThis.restorePlanWrites());
  await saveButton(page).click();
  await expect(saveButton(page)).toHaveText('Saved');
  await expect(page.getByRole('button', { name: 'Download plan', exact: true })).toHaveCount(0);
  await page.reload();
  await expect(page.getByRole('heading', { name: 'Keep me', exact: true })).toBeVisible();
  await expect(page.getByRole('combobox', { name: 'Plan type' })).toContainText('Multi-day trip');
});

test('a new route is stored only at Save, with the name of the window, and a reload continues its changes', async ({ page }) => {
  await start(page);
  await expect(saveButton(page)).toBeDisabled();
  const canvas = page.locator('.maplibregl-canvas');
  for (const [fx, choice] of [[0.3, /Start here/], [0.7, /Finish here/]]) {
    const { x, y, width, height } = await canvas.boundingBox();
    await page.mouse.click(x + width * fx, y + height * 0.5);
    await page.getByRole('button', { name: choice }).click();
  }
  await expect(saveButton(page)).toBeEnabled();
  await expect.poll(() => records(page, 'drafts')).toHaveLength(1);
  await page.reload();
  await expect(saveButton(page)).toBeEnabled();
  await expect(page.getByRole('button', { name: 'Undo', exact: true })).toBeEnabled();
  expect(await records(page, 'routes')).toEqual([]);

  await saveButton(page).click();
  await expect(dialog(page).getByRole('heading')).toHaveText('Save route');
  const name = dialog(page).getByRole('textbox', { name: 'Name' });
  await expect(name).toBeFocused();
  await name.fill('');
  await expect(dialog(page).getByRole('button', { name: 'Save', exact: true })).toBeDisabled();
  await name.fill('Lake test');
  await name.press('Enter');
  await expect(saveButton(page)).toHaveText('Saved');
  await expect(saveButton(page)).toBeDisabled();
  await expect(saveButton(page)).toBeFocused();
  await expect(page.getByRole('heading', { name: 'Lake test', exact: true })).toBeVisible();
  expect((await records(page, 'routes')).map(route => [route.name, route.trip.mode])).toEqual([['Lake test', 'route']]);
  expect(await records(page, 'drafts')).toEqual([]);

  await changeType(page, 'Multi-day trip');
  await expect(saveButton(page)).toHaveText('Save');
  await expect.poll(async () => (await records(page, 'drafts')).map(draft => draft.trip.mode)).toEqual(['trip']);
  await page.reload();
  await expect(page.getByRole('combobox', { name: 'Plan type' })).toContainText('Multi-day trip');
  await expect(saveButton(page)).toHaveText('Save');
  expect((await records(page, 'routes'))[0].trip.mode).toBe('route');
  await page.getByRole('button', { name: 'Undo', exact: true }).click();
  await expect(saveButton(page)).toHaveText('Saved');
  await expect.poll(() => records(page, 'drafts')).toEqual([]);
});

test('before another route opens, the question keeps the route at Cancel, its draft at Discard, and stores it at Save', async ({ page }) => {
  await start(page);
  await page.getByRole('button', { name: 'My plans', exact: true }).click();
  await importFile(page, planFile('First'));
  await importFile(page, planFile('Second'));
  await row(page, 'First').getByRole('button', { name: /^First/ }).click();
  await expect(page.getByRole('heading', { name: 'First', exact: true })).toBeVisible();
  await changeType(page, 'Multi-day trip');
  const openSecond = async () => {
    await page.getByRole('button', { name: 'My plans', exact: true }).click();
    await row(page, 'Second').getByRole('button', { name: /^Second/ }).click();
    await expect(dialog(page).getByRole('heading')).toHaveText('Save changes to ‘First’?');
    await expect(dialog(page).getByRole('button', { name: 'Save', exact: true })).toBeFocused();
  };
  const first = async () => (await records(page, 'routes')).find(route => route.name === 'First');

  await openSecond();
  await page.keyboard.press('Escape');
  await expect(dialog(page)).toHaveCount(0);
  await expect(page.getByRole('heading', { name: 'First', exact: true })).toBeVisible();
  await expect(saveButton(page)).toHaveText('Save');

  await openSecond();
  await dialog(page).getByRole('button', { name: 'Discard', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Second', exact: true })).toBeVisible();
  expect((await first()).trip.mode).toBe('route');
  expect((await records(page, 'drafts')).map(draft => draft.trip.mode)).toEqual(['trip']);

  await page.getByRole('button', { name: 'My plans', exact: true }).click();
  await row(page, 'First').getByRole('button', { name: /^First/ }).click();
  await expect(saveButton(page)).toHaveText('Saved');
  await changeType(page, 'Multi-day trip');
  await openSecond();
  await dialog(page).getByRole('button', { name: 'Save', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Second', exact: true })).toBeVisible();
  expect((await first()).trip.mode).toBe('trip');
  expect(await records(page, 'drafts')).toEqual([]);
});

test('"Save a copy…" stores the changes as a new route, and "Discard changes" has Undo', async ({ page }) => {
  await start(page);
  await page.getByRole('button', { name: 'My plans', exact: true }).click();
  await importFile(page, planFile('First'));
  await library(page).getByRole('button', { name: 'Close My plans' }).click();
  await changeType(page, 'Multi-day trip');
  await more(page, 'Save a copy…');
  await expect(dialog(page).getByRole('heading')).toHaveText('Save a copy');
  await expect(dialog(page).getByRole('textbox', { name: 'Name' })).toHaveValue('First (copy)');
  await dialog(page).getByRole('button', { name: 'Save copy', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'First (copy)', exact: true })).toBeVisible();
  await expect(saveButton(page)).toHaveText('Saved');
  await expect(page.getByRole('button', { name: 'More ▾', exact: true })).toBeFocused();
  expect((await records(page, 'routes')).map(route => [route.name, route.trip.mode]).sort()).toEqual([['First (copy)', 'trip'], ['First', 'route']]);

  await changeType(page, 'Route');
  await more(page, 'Discard changes');
  await expect(page.getByRole('combobox', { name: 'Plan type' })).toContainText('Multi-day trip');
  await expect(saveButton(page)).toHaveText('Saved');
  await expect(page.locator('.status-line')).toContainText('Changes discarded');
  await page.locator('.status-line').getByRole('button', { name: 'Undo', exact: true }).click();
  await expect(page.getByRole('combobox', { name: 'Plan type' })).toContainText('Route');
  await expect(saveButton(page)).toHaveText('Save');
});

