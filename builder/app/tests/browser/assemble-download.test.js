/**
 * The builder's download journey, in a real browser, on the shipped `dist/web` build.
 *
 * Pick the fixture region from a digest-pinned loopback catalog, let the app fetch and verify the
 * cells into OPFS, assemble them in the real worker through the real wasm bridge, and save the
 * `.obcm`. The downloaded bytes must be the checked-in `expected/map.obcm` — the file
 * `cargo run -p obcm-assemble` wrote from the same cells — so this is the third host held to those
 * bytes, after the native test and the Node bridge test, and the only one that runs the worker,
 * OPFS and the download path that ship.
 *
 * Nothing leaves loopback. The basemap is presentational and is answered with a local pixel; any
 * other off-origin request is aborted and fails the test.
 */

import { expect, test } from '@playwright/test';
import { createHash } from 'node:crypto';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const REPO = join(dirname(fileURLToPath(import.meta.url)), '../../../..');
const EXPECTED = join(REPO, 'apps/obc-web-assemble/tests/fixture/expected/map.obcm');
const CATALOG_LOG = join(REPO, '.artifacts/web-builder/catalog.jsonl');
const REGION = 'Bridge Fixture';
/** The presentational basemap's host. Its tiles are decoration; the journey needs none of them. */
const BASEMAP = 'tile.openstreetmap.org';
/** One transparent pixel, so a served tile costs nothing and logs no load failure. */
const PIXEL = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==',
  'base64',
);

const sha256 = (bytes) => createHash('sha256').update(bytes).digest('hex');

test('assembles the fixture region in the tab and downloads the pinned map', async ({ page }, testInfo) => {
  const diagnostics = [`browser: ${page.context().browser().version()}`];
  const errors = [];
  const offOrigin = [];
  page.on('console', (message) => {
    diagnostics.push(`${message.type()}: ${message.text()}`);
    if (message.type() === 'error') errors.push(message.text());
  });
  page.on('pageerror', (error) => {
    diagnostics.push(`pageerror: ${error.stack}`);
    errors.push(error.message);
  });
  page.on('worker', (worker) => diagnostics.push(`worker: ${worker.url()}`));

  await page.route('**/*', (route) => {
    const url = new URL(route.request().url());
    if (url.hostname === '127.0.0.1' || url.hostname === 'localhost') return route.continue();
    offOrigin.push(url.href);
    if (url.hostname === BASEMAP) {
      return route.fulfill({ status: 200, contentType: 'image/png', body: PIXEL });
    }
    return route.abort();
  });

  let failure;
  try {
    await page.goto('/');

    const search = page.getByLabel('Search regions');
    await expect(search).toBeVisible();
    await search.fill('Bridge');
    // The price is in the label and the formatter owns its spelling, so match the prefix.
    await page.locator(`[aria-label^="Add ${REGION} ("]`).click();
    await expect(page.locator(`[aria-label="${REGION} is already in the map"]`)).toBeVisible();

    const download = page.getByRole('button', { name: 'Download map' });
    // Enabled only once the ledger is final and the memory projection has been admitted.
    await expect(download).toBeEnabled();
    const [saved] = await Promise.all([page.waitForEvent('download'), download.click()]);

    await expect(page.locator('.done')).toBeVisible();
    // The shipped path, not the fallback: cells read out of OPFS through sync access handles and
    // the map written into OPFS from inside the wasm call. A browser without them buffers both,
    // which is a different code path and would pass every other assertion here.
    await expect(page.locator('.done')).toContainText(/cells streamed\s*·\s*map disk/);
    expect(saved.suggestedFilename()).toBe(`${REGION}.obcm`);

    const want = await readFile(EXPECTED);
    const got = await readFile(await saved.path());
    diagnostics.push(`downloaded ${got.byteLength} B, sha256 ${sha256(got)}`);
    expect(got.byteLength).toBe(want.byteLength);
    expect(sha256(got)).toBe(sha256(want));

    // Nothing in the catalog, ledger or parts list reported a failure it recovered from.
    await expect(page.locator('.catalog-error, .ledger .error, .parts .retry')).toHaveCount(0);

    // The server's log: its first record names what it published, the rest are the requests.
    const records = (await readFile(CATALOG_LOG, 'utf8')).trim().split('\n').map((l) => JSON.parse(l));
    const requested = new Set(records.slice(1).filter((r) => r.kind === 'object').map((r) => r.path));
    expect([...records[0].served].filter((path) => !requested.has(path))).toEqual([]);
    expect(records.slice(1).filter((r) => r.kind === 'missing' && r.path.startsWith('/catalog/'))).toEqual([]);
  } catch (error) {
    failure = error;
  }

  await mkdir(testInfo.outputDir, { recursive: true });
  const path = testInfo.outputPath('browser-diagnostics.log');
  await writeFile(path, diagnostics.join('\n'));
  await testInfo.attach('browser-diagnostics', { path, contentType: 'text/plain' });
  if (failure) throw failure;

  // Last, after every other await in this test, because these two accumulate: an off-origin request
  // or a console error raised while the page settled after the download is still a failure, and
  // asserting them mid-journey would only cover what had happened by then.
  expect([...new Set(offOrigin.map((href) => new URL(href).hostname))].filter((h) => h !== BASEMAP)).toEqual([]);
  expect(errors).toEqual([]);
});
