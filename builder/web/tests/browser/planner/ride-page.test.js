import { readFileSync } from 'node:fs';
import { expect, test } from '@playwright/test';
import { initSync, obc_fit_write_ride } from '../../../src/lib/core/pkg/obc_builder_bridge.js';
import { CORS_PREFLIGHT, respond } from './stubs.js';

initSync({ module: readFileSync(new URL('../../../src/lib/core/pkg/obc_builder_bridge_bg.wasm', import.meta.url)) });

const start = Date.UTC(2026, 9, 24, 7) / 1000, key = '20261024T070000Z';
// 200 samples 10 s apart, about 50 m each to the north: 5 km down 20 m at 140 bpm and 200 W, then 5 km up 200 m at 160 bpm and 260 W.
const samples = Array.from({ length: 200 }, (_, i) => {
  const up = i >= 100;
  return [start + 10 * i, 46_900_000 + 450 * i, 8_000_000, i ? 0 : 1, up ? 980 + (i - 99) * 2 : 1000 - 0.2 * i, 5, up ? 160 : 140, 85, up ? 260 : 200, 10];
});
const file = obc_fit_write_ride('bike', Float64Array.from(samples.flat()));
const ride = {
  key, name: 'Above Engelberg', source: 'file', format: 'fit', maxHr: null, ftp: null,
  summary: { line: [8, 46.9, 8, 46.945, 8, 46.98955], heights: [1000, 980, 1180], bounds: [8, 46.9, 8, 46.98955], lengthM: 9958, climbM: 200,
    highestM: 1180, seconds: 1990, movingS: 1990, start, bike: 'touring' },
};
const text = async download => readFileSync(await download.path(), 'utf8');

test('the ride page shows a stored ride with its numbers and zones, renames, exports, rides it again and deletes', async ({ page }) => {
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.route(url => url.hostname !== '127.0.0.1', request => {
    if (request.request().method() === 'OPTIONS') return request.fulfill(CORS_PREFLIGHT);
    const response = respond(request.request());
    return response ? request.fulfill(response) : request.abort();
  });
  await page.goto(`/planner.html#/ride/${key}`);
  await expect(page.getByText('This ride is not in the library.')).toBeVisible();

  await page.goto('/planner.html?cover=none');
  await page.waitForFunction(() => window.coverTest);
  await page.evaluate(([record, bytes]) => window.coverTest.library.addRide(record, new Blob([new Uint8Array(bytes)])), [ride, [...file]]);
  await page.goto(`/planner.html#/ride/${key}`);
  const facts = page.locator('.facts');
  await expect(page.getByRole('heading', { name: 'Above Engelberg', level: 1 })).toBeVisible();
  await expect(page.getByText('24 October 2026')).toBeVisible();
  for (const [label, value] of [['Distance', '10.0 km'], ['Climb', '200 m'], ['Time moving', '0h 33m'], ['Speed', '18.0 km/h']]) {
    await expect(facts.locator('dl > div').filter({ hasText: label })).toContainText(value);
  }
  // The moving steps weigh 99 × 10 s on the flat and 100 × 10 s up.
  await expect(facts.locator('.sensor').filter({ hasText: 'Heart rate' })).toContainText('150max 160');
  await expect(facts.locator('.sensor').filter({ hasText: 'Power' })).toContainText('230 Wmax 260');
  await expect(facts.locator('.sensor').filter({ hasText: 'Cadence' })).toContainText('85');
  await expect(facts.getByText('Set your maximum heart rate in Settings to see zones.')).toBeVisible();
  await expect(facts.getByText('Set your threshold power in Settings to see zones.')).toBeVisible();
  await expect(facts.locator('.rows li')).toHaveCount(1);

  // With no values in the ride, the zones use the settings.
  await page.evaluate(() => localStorage.setItem('obc-settings', JSON.stringify({ units: 'km', maxHr: { value: 185, at: 0, device: false }, ftp: { value: 250, at: 0, device: false } })));
  await page.reload();
  await expect(facts.getByRole('img', { name: /Z4 · 148–166 bpm · 0h 17m/ })).toBeVisible();
  await expect(facts.getByRole('img', { name: /Z5 · 263\+ W/ })).toBeVisible();

  await page.getByRole('button', { name: 'Rename' }).click();
  await page.getByRole('textbox', { name: 'Ride name' }).fill('Trübsee');
  await page.keyboard.press('Enter');
  await expect(page.getByRole('heading', { name: 'Trübsee', level: 1 })).toBeVisible();

  const exported = async label => {
    await page.getByRole('button', { name: 'Export ▾' }).click();
    const [download] = await Promise.all([page.waitForEvent('download'), page.getByRole('button', { name: label }).click()]);
    return download;
  };
  const fit = await exported('FIT file');
  expect(fit.suggestedFilename()).toBe('Trübsee.fit');
  expect(readFileSync(await fit.path()).equals(Buffer.from(file))).toBe(true);
  const gpx = await text(await exported('GPX track'));
  expect(gpx).toContain('<time>2026-10-24T07:33:10.000Z</time>');
  const csv = (await text(await exported('Table of the data (CSV)'))).split('\n');
  expect(csv[0]).toBe('time,latitude,longitude,elevation_m,distance_m,speed_m_s,heart_rate_bpm,cadence_rpm,power_w,temperature_c');
  expect(csv[1]).toBe('2026-10-24T07:00:00.000Z,46.900000,8.000000,1000,0,5,140,85,200,10');

  await page.getByRole('button', { name: 'Ride it again' }).click();
  await expect(page).toHaveURL(/#\/route\/file-[0-9a-f]{32}$/);
  await expect(page.getByRole('heading', { name: 'Trübsee', level: 1 })).toBeVisible();
  const route = page.url();
  await page.goBack();
  await page.getByRole('button', { name: 'Ride it again' }).click();
  await expect(page).toHaveURL(route);
  await page.goBack();

  await facts.getByRole('button', { name: 'Delete' }).click();
  await page.getByRole('dialog', { name: 'Delete this ride?' }).getByRole('button', { name: 'Delete' }).click();
  await expect(page).toHaveURL(/#\/routes\/ridden$/);
  await page.evaluate(k => { location.hash = `#/ride/${k}`; }, key);
  await expect(page.getByText('This ride is not in the library.')).toBeVisible();

  await page.goto('/planner.html?cover=none');
  await page.waitForFunction(() => window.coverTest);
  expect(await page.evaluate(() => window.coverTest.library.routes().then(routes => routes.length))).toBe(1);
  expect(errors).toEqual([]);
});
