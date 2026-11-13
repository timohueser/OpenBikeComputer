import { readFileSync } from 'node:fs';
import { crc32, deflateRawSync, gzipSync } from 'node:zlib';
import { expect, test } from '@playwright/test';
import { initSync, obc_fit_write_course, obc_fit_write_ride } from '../../../src/lib/core/pkg/obc_builder_bridge.js';

initSync({ module: readFileSync(new URL('../../../src/lib/core/pkg/obc_builder_bridge_bg.wasm', import.meta.url)) });

const start = Date.UTC(2026, 7, 1, 7) / 1000;
const line = (lon, n) => Array.from({ length: n }, (_, i) => [lon + i * 0.001, 47.9 + 0.001 * Math.sin(i / 5)]);

const gpx = (points, times) => `<?xml version="1.0"?><gpx><trk><trkseg>${points.map(([lon, lat], i) =>
  `<trkpt lat="${lat}" lon="${lon}"><ele>${400 + i}</ele>${times ? `<time>${new Date((times + i * 30) * 1000).toISOString()}</time>` : ''}</trkpt>`).join('')}</trkseg></trk></gpx>`;

function fit(points, t0) {
  const values = new Float64Array(points.length * 10).fill(NaN);
  points.forEach(([lon, lat], i) => values.set([t0 + i * 30, Math.round(lat * 1e6), Math.round(lon * 1e6), i ? 0 : 1], i * 10));
  return Buffer.from(obc_fit_write_ride('bike', values));
}

/** A FIT course with no waypoints; the points are microdegrees and metres. */
const course = points => Buffer.from(obc_fit_write_course('Course', Float64Array.from(points.flatMap(([lon, lat], i) => [Math.round(lat * 1e6), Math.round(lon * 1e6), 400 + i])), new Float64Array(0), [], 3600));

/** A ZIP file of deflated entries. */
function zip(parts) {
  const files = [], central = [];
  let offset = 0;
  for (const [name, data] of parts) {
    const path = Buffer.from(name), packed = deflateRawSync(data), local = Buffer.alloc(30), header = Buffer.alloc(46);
    local.writeUInt32LE(0x04034b50, 0);
    header.writeUInt32LE(0x02014b50, 0);
    for (const [b, at] of [[local, 4], [header, 6]]) {
      [[2, 0x0800], [4, 8]].forEach(([field, value]) => b.writeUInt16LE(value, at + field));
      [[10, crc32(data)], [14, packed.length], [18, data.length]].forEach(([field, value]) => b.writeUInt32LE(value, at + field));
      b.writeUInt16LE(path.length, at + 22);
    }
    header.writeUInt32LE(offset, 42);
    files.push(local, path, packed);
    central.push(header, path);
    offset += 30 + path.length + packed.length;
  }
  const directory = Buffer.concat(central), end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  [[8, parts.length], [10, parts.length]].forEach(([at, value]) => end.writeUInt16LE(value, at));
  [[12, directory.length], [16, offset]].forEach(([at, value]) => end.writeUInt32LE(value, at));
  return Buffer.concat([...files, directory, end]);
}

const files = [
  { name: 'ride.gpx', mimeType: 'application/gpx+xml', buffer: Buffer.from(gpx(line(8, 60), start)) },
  { name: 'route.gpx', mimeType: 'application/gpx+xml', buffer: Buffer.from(gpx(line(8.2, 80))) },
  { name: 'course.fit', mimeType: 'application/octet-stream', buffer: course(line(8.6, 40)) },
  { name: 'old.tcx', mimeType: 'application/xml', buffer: Buffer.from('<TrainingCenterDatabase/>') },
  { name: 'export.zip', mimeType: 'application/zip', buffer: zip([['activities/1.fit.gz', gzipSync(fit(line(8.4, 50), start + 86400))]]) },
];

async function open(page) {
  await page.goto('/planner.html#/routes');
  await page.getByRole('button', { name: 'Add files' }).click();
  await expect(page.getByRole('heading', { name: 'Add files' })).toBeVisible();
  await page.getByLabel('Choose files').setInputFiles(files);
  await expect(page.getByText('5 files read')).toBeVisible();
}

test('adds the rides and the routes of files and a ZIP file once, and gives the reason for a file it cannot read', async ({ page }) => {
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await open(page);
  await expect(page.locator('dl > div')).toHaveText(['rides2', 'planned routes2', 'are in your library already0', 'not readable1']);
  await expect(page.getByRole('listitem').filter({ hasText: 'old.tcx' })).toContainText('not readable: TCX files cannot be read');
  // A GPX file with times can also be a planned route.
  const form = page.getByRole('radiogroup', { name: 'ride: ride or planned route' });
  await form.getByRole('radio', { name: 'planned route' }).click();
  await expect(page.locator('dl > div')).toHaveText(['rides1', 'planned routes3', 'are in your library already0', 'not readable1']);
  await form.getByRole('radio', { name: 'ride', exact: true }).click();
  await expect(page.locator('dl > div')).toHaveText(['rides2', 'planned routes2', 'are in your library already0', 'not readable1']);
  await page.getByRole('button', { name: 'Add 4 to the library' }).click();
  await expect(page.getByRole('status')).toHaveText('Added 2 rides and 2 planned routes.');
  await page.keyboard.press('Escape');
  // The Routes page shows the planned routes only.
  await expect(page.getByRole('radio', { name: 'Planned 2' })).toBeChecked();
  await expect(page.locator('ul').getByRole('button')).toHaveCount(2);

  await page.goto('/planner.html?cover=none');
  await page.waitForFunction(() => window.coverTest);
  const stored = await page.evaluate(async () => {
    const { library } = window.coverTest;
    return { rides: (await library.rides()).map(ride => ride.key), routes: (await library.routes()).map(route => [route.id.slice(0, 5), route.summary.days, route.trip.mode]) };
  });
  expect(stored).toEqual({ rides: ['20260801T070000Z', '20260802T070000Z'], routes: [['file-', 1, 'route'], ['file-', 1, 'route']] });

  await open(page);
  await expect(page.locator('dl > div')).toHaveText(['rides0', 'planned routes0', 'are in your library already4', 'not readable1']);
  await expect(page.getByRole('button', { name: 'Add 0 to the library' })).toBeDisabled();
  expect(errors).toEqual([]);
});
