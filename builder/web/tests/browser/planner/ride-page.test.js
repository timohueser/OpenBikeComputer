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

// A ride from a GPX file, with no sensor: 20 points 10 s apart, about 50 m each to the north.
const plainKey = '20261025T070000Z';
const plain = {
  key: plainKey, name: 'No sensors', source: 'file', format: 'gpx', maxHr: null, ftp: null,
  summary: { line: [8, 46.9, 8, 46.90855], heights: [1000, 1019], bounds: [8, 46.9, 8, 46.90855], lengthM: 951, climbM: 19,
    highestM: 1019, seconds: 190, movingS: 190, start: start + 86_400, bike: 'touring' },
};
const plainFile = `<?xml version="1.0"?><gpx version="1.1" creator="test" xmlns="http://www.topografix.com/GPX/1/1"><trk><trkseg>${
  Array.from({ length: 20 }, (_, i) => `<trkpt lat="${(46.9 + 0.00045 * i).toFixed(5)}" lon="8"><ele>${1000 + i}</ele><time>${new Date((start + 86_400 + 10 * i) * 1000).toISOString()}</time></trkpt>`).join('')
}</trkseg></trk></gpx>`;

async function answer(page) {
  await page.route(url => url.hostname !== '127.0.0.1', request => {
    if (request.request().method() === 'OPTIONS') return request.fulfill(CORS_PREFLIGHT);
    const response = respond(request.request());
    return response ? request.fulfill(response) : request.abort();
  });
}

test('the ride page shows a stored ride with its numbers and zones, renames, exports, rides it again and deletes', async ({ page }) => {
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await answer(page);
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
  await expect(facts.locator('.climbs li')).toHaveCount(1);

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
    await page.getByRole('button', { name: 'More ▾' }).click();
    const [download] = await Promise.all([page.waitForEvent('download'), page.getByRole('button', { name: label }).click()]);
    return download;
  };
  const fit = await exported('Export FIT file');
  expect(fit.suggestedFilename()).toBe('Trübsee.fit');
  expect(readFileSync(await fit.path()).equals(Buffer.from(file))).toBe(true);
  const gpx = await text(await exported('Export GPX track'));
  expect(gpx).toContain('<time>2026-10-24T07:33:10.000Z</time>');
  const csv = (await text(await exported('Export table of the data (CSV)'))).split('\n');
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

  await page.getByRole('button', { name: 'More ▾' }).click();
  await page.getByRole('navigation', { name: 'Actions' }).getByRole('button', { name: 'Delete' }).click();
  await page.getByRole('dialog', { name: 'Delete this ride?' }).getByRole('button', { name: 'Delete' }).click();
  await expect(page).toHaveURL(/#\/routes\/ridden$/);
  await page.evaluate(k => { location.hash = `#/ride/${k}`; }, key);
  await expect(page.getByText('This ride is not in the library.')).toBeVisible();

  await page.goto('/planner.html?cover=none');
  await page.waitForFunction(() => window.coverTest);
  expect(await page.evaluate(() => window.coverTest.library.routes().then(routes => routes.length))).toBe(1);
  expect(errors).toEqual([]);
});

test('the lines of a ride: a row for each line with values, the pointer, and a part by a drag with its numbers', async ({ page }) => {
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await answer(page);
  await page.goto('/planner.html?cover=none');
  await page.waitForFunction(() => window.coverTest);
  await page.evaluate(() => localStorage.setItem('obc-settings', JSON.stringify({ units: 'km', maxHr: { value: 185, at: 0, device: false }, ftp: { value: 250, at: 0, device: false } })));
  await page.evaluate(async ([record, bytes, other, gpx]) => {
    await window.coverTest.library.addRide(record, new Blob([new Uint8Array(bytes)]));
    await window.coverTest.library.addRide(other, new Blob([gpx]));
  }, [ride, [...file], plain, plainFile]);

  // A ride with no sensor has the rows of the elevation and the speed only, and no sensor in the left card.
  await page.goto(`/planner.html#/ride/${plainKey}`);
  const facts = page.locator('.facts'), lanes = page.locator('.lanes'), chips = page.getByRole('group', { name: 'Shown lines' }).getByRole('button');
  await expect(chips).toHaveText(['Elevation', 'Speed']);
  await expect(lanes.locator('.label')).toHaveText(['Elevation', 'Speed18.0 km/h']);
  await expect(facts.locator('.sensor')).toHaveCount(0);
  // The arrow keys move the pointer both ways on a ride whose samples are further apart than a key step.
  await lanes.locator('.over').focus();
  for (let i = 0; i < 6; i++) await page.keyboard.press('ArrowRight');
  const right = await lanes.locator('.chip').textContent();
  for (let i = 0; i < 3; i++) await page.keyboard.press('ArrowLeft');
  await expect(lanes.locator('.chip')).not.toHaveText(right);

  // A ride with sensors: a row for each line, the cadence off; the average under the name, the scale after the row.
  await page.goto(`/planner.html#/ride/${key}`);
  await expect(chips).toHaveText(['Elevation', 'Speed', 'Heart rate', 'Power', 'Cadence']);
  await expect(lanes.locator('.label')).toHaveText(['Elevation', 'Speed18.0 km/h', 'Heart rate150 bpm', 'Power230 W']);
  await expect(lanes.locator('.scale').nth(2)).toHaveText('160 bpm140 bpm');
  await chips.filter({ hasText: 'Cadence' }).click();
  await expect(lanes.locator('.label').last()).toHaveText('Cadence85 rpm');
  // A hidden line keeps its zones in the left card.
  await chips.filter({ hasText: 'Heart rate' }).click();
  await expect(lanes.locator('.label')).toHaveCount(4);
  await expect(facts.getByRole('img', { name: /Z4 · 148–166 bpm · 0h 17m/ })).toBeVisible();
  await chips.filter({ hasText: 'Heart rate' }).click();

  // The pointer shows the value of each line at its place.
  const box = await lanes.locator('.over').boundingBox(), y = box.y + box.height / 2;
  await page.mouse.move(box.x + box.width * 0.25, y);
  await expect(lanes.locator('.chip')).toHaveText('2.5 km');
  await expect(lanes.locator('.lane').nth(2)).toHaveText('140 bpm');

  // A drag over the second half selects the climb: the header shows its distance, the left card its numbers beside
  // those of the whole ride, and the map marks it.
  await page.mouse.move(box.x + box.width * 0.5, y);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width * 0.75, y);
  await page.mouse.move(box.x + box.width - 1, y);
  await page.mouse.up();
  await expect(page.locator('.lines .part')).toContainText('km 5.0 to km 10.0 · 5.0 km');
  await expect(facts.locator('thead th')).toHaveText(['Selected', 'Whole ride']);
  for (const [label, selected, whole] of [['Distance', '5.0 km', '10.0 km'], ['Time moving', '0h 17m', '0h 33m'], ['Speed', '18.0 km/h', '18.0 km/h'],
    ['Heart rate', '160', '150'], ['Power', '260 W', '230 W'], ['Cadence', '85', '85']]) {
    await expect(facts.locator('tbody tr').filter({ hasText: label }).locator('td')).toHaveText([selected, whole]);
  }
  // The zones are those of the part: all of it is in Z4.
  await expect(facts.getByRole('img', { name: /Z3 · 130–147 bpm · 0h 0m, Z4 · 148–166 bpm · 0h 17m/ })).toBeVisible();
  await expect(facts.locator('.climbs')).toHaveCount(0);
  await page.keyboard.press('Escape');
  await expect(page.locator('.lines .part')).toHaveCount(0);
  await expect(facts.locator('dl > div').filter({ hasText: 'Distance' })).toContainText('10.0 km');

  // The elevation row fills the climb; a click on it selects the climb, a click on another row clears the part.
  const click = async (row, at) => {
    const lane = await lanes.locator('.lane').nth(row).boundingBox();
    await page.mouse.click(lane.x + lane.width * at, lane.y + lane.height / 2);
  };
  await expect(lanes.locator('.lane').first().locator('path[clip-path]')).toHaveCount(1);
  await click(0, 0.75);
  await expect(page.locator('.lines .part')).toContainText('km 5.1 to km 9.9 · 4.9 km');
  await expect(facts.locator('tbody tr').filter({ hasText: 'Heart rate' }).locator('td')).toHaveText(['160', '150']);
  await click(1, 0.75);
  await expect(page.locator('.lines .part')).toHaveCount(0);
  expect(errors).toEqual([]);
});
