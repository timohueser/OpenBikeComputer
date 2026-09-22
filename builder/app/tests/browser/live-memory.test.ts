/**
 * The browser memory gate: a country-scale assembly must not hold more wasm memory than the
 * projection it was admitted on.
 *
 * The builder refuses a selection whose projected peak does not fit a tab. That projection is
 * arithmetic, and arithmetic cannot fail when the assembler buffers a whole map anyway — the
 * estimator would still return its number and the preflight would still admit the run. This suite
 * is the measurement that can fail: it assembles the largest published region in the shipped
 * `dist/web` build, reads the linear memory the worker reports at `done`, and holds it against the
 * estimator's own engine term for the same selection and the same sort budget.
 *
 * It needs the live catalogue, so it is `live`: the published objects are the only input large
 * enough that buffering one would show. The fixture region in `assemble-download.test.js` is five
 * cells; a run that buffered all of it would still sit under the estimate.
 *
 * Two things about the browser are not the runner's defaults, and both are about the runner rather
 * than the product. Chromium runs with CORS off, because the catalogue's CDN answers
 * `Access-Control-Allow-Origin` for the deployed origins only and this page is served from
 * loopback. And the run gets an ordinary browser profile instead of the runner's throwaway
 * context, because Chromium caps an incognito context's storage at 3 GiB while a profile on the
 * same machine is given several times that, and a country-scale run holds its cells, its map and
 * its spill on disk at once. Nothing else is changed: the build, the worker, the wasm engine, the
 * OPFS seams and the verify pass are the shipping ones, and `assemble-download.test.js` covers the
 * same journey under an enforcing browser.
 */

import { chromium, expect, test } from '@playwright/test';
import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { mkdir, mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

import { estimateMemory, initAssemble } from '../../src/lib/assemble/bridge';

const HERE = dirname(fileURLToPath(import.meta.url));
const WASM = join(HERE, '../../src/lib/assemble/pkg/obc_web_assemble_bg.wasm');
const DOWNLOAD_STEP = join(HERE, '../../src/components/coverage/DownloadStep.svelte');

/** The host the catalogue must come from: a loopback fixture would make this gate meaningless. */
const LIVE_HOST = 'maps.openbikecomputer.com';

/** The presentational basemap's host. Its tiles are decoration; the measurement needs none. */
const BASEMAP = 'tile.openstreetmap.org';
/** One transparent pixel, so a served tile costs nothing and logs no load failure. */
const PIXEL = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==',
  'base64',
);

/**
 * How far over its own engine term the run may land.
 *
 * The engine term is a linear fit, and the wasm allocator rounds a run's arenas up over it: a
 * country-scale run measures 1.062 of the term, to the byte, on repeated runs. The margin covers
 * that rounding and nothing else. It is not slack for growth — a run that buffers what the
 * estimator says it streams misses by hundreds of megabytes, not by ten percent.
 */
const MARGIN = 1.1;

/** Below this the gate cannot fail, because buffering the whole selection would still fit. */
const MEANINGFUL_BYTES = 512 * 1024 * 1024;

/** The engine's sort budget the download screen runs with, read from it so the two cannot drift. */
async function sortBudgetBytes(): Promise<number> {
  const source = await readFile(DOWNLOAD_STEP, 'utf8');
  const match = /SORT_BUDGET_BYTES = (\d+) \* 1024 \* 1024/.exec(source);
  expect(match, 'DownloadStep.svelte no longer states SORT_BUDGET_BYTES the way this suite reads it').toBeTruthy();
  return Number(match![1]) * 1024 * 1024;
}

function sha256(path: string): Promise<string> {
  const hash = createHash('sha256');
  return new Promise((resolve, reject) => {
    createReadStream(path).on('error', reject).on('data', (c) => hash.update(c)).on('end', () => resolve(hash.digest('hex')));
  });
}

interface Band {
  id: string;
  role: string;
}

interface Region {
  name: string;
  bytes: number;
  bytes_by_band: Record<string, number>;
  terrain?: { bytes: number };
}

test('holds a country-scale assembly to the memory its estimate promised', async ({}, testInfo) => {
  const options = testInfo.project.use;
  const profileDir = await mkdtemp(join(tmpdir(), 'obc-live-memory-'));
  const context = await chromium.launchPersistentContext(profileDir, {
    args: options.launchOptions?.args,
    viewport: options.viewport,
  });
  const page = context.pages()[0] ?? (await context.newPage());
  page.setDefaultTimeout(options.actionTimeout ?? 30_000);

  const notes: string[] = [];
  const errors: string[] = [];
  page.on('console', (m) => {
    notes.push(`${m.type()}: ${m.text()}`);
    if (m.type() === 'error') errors.push(m.text());
  });
  page.on('pageerror', (e) => {
    notes.push(`pageerror: ${e.stack}`);
    errors.push(e.message);
  });
  // Only the basemap is intercepted. Everything else, the catalogue objects above all, reaches
  // the network untouched: routing them would put 890 MB through the test protocol.
  await page.route(`https://${BASEMAP}/**`, (route) =>
    route.fulfill({ status: 200, contentType: 'image/png', body: PIXEL }),
  );

  // Both subscribed before the page loads, because each is posted once and the catalogue lands
  // while `goto` is still resolving.
  const profile = page
    .waitForEvent('console', {
      predicate: (m) => m.text().startsWith('[assemble] run'),
      timeout: testInfo.timeout,
    })
    .then((m) => m.args()[1].jsonValue() as Promise<{ wasmMemoryBytes: number }>);
  // The catalogue the tab actually read, taken from its own response: the suite never names a URL,
  // so it cannot measure a different catalogue than the one the build points at.
  const rootResponse = page.waitForResponse((r) => r.url().endsWith('/catalog.json') && r.ok());

  let failure: unknown;
  try {
    await page.goto(String(options.baseURL));
    notes.push(`browser: ${await page.evaluate(() => navigator.userAgent)}`);

    const root = await rootResponse;
    expect(new URL(root.url()).hostname, 'this build is not pointed at the published catalogue').toBe(LIVE_HOST);
    const catalog = JSON.parse(await root.text()) as {
      generated_at: string;
      schema: { bands: Band[] };
      regions: Region[];
    };
    notes.push(`catalog: ${root.url()} generated_at ${catalog.generated_at}`);

    // The largest published region, priced the way `ledger.ts` prices one.
    const region = catalog.regions.reduce((a, b) => (b.bytes > a.bytes ? b : a));
    const core = catalog.schema.bands.find((b) => b.role === 'core')!;
    const terrainBytes = region.terrain?.bytes ?? 0;
    const totalCellBytes = region.bytes + terrainBytes;
    expect(totalCellBytes, 'the published catalogue is too small for this gate to be able to fail').toBeGreaterThan(
      MEANINGFUL_BYTES,
    );

    await initAssemble(await readFile(WASM));
    const estimate = await estimateMemory(
      region.bytes_by_band[core.id],
      totalCellBytes,
      terrainBytes,
      await sortBudgetBytes(),
      { inputOnDisk: true, outputSunk: true },
    );
    notes.push(
      `selection: ${region.name}, ${totalCellBytes} B (${terrainBytes} B terrain)`,
      `estimate: engine ${estimate.engineBytes} B, peak ${estimate.peakBytes} B, fits ${estimate.fits}`,
    );

    const search = page.getByLabel('Search regions');
    await expect(search).toBeVisible();
    await search.fill(region.name);
    await page.locator(`[aria-label^="Add ${region.name} ("]`).click();
    await expect(page.locator(`[aria-label="${region.name} is already in the map"]`)).toBeVisible();

    const download = page.getByRole('button', { name: 'Download map' });
    // Enabled only once the ledger is final and the memory projection has been admitted.
    await expect(download).toBeEnabled();
    const saving = page.waitForEvent('download', { timeout: testInfo.timeout });
    await download.click();
    // A refused or failed run never produces a download, so the failure is read off the screen
    // rather than waited out. `.line.warn` is the download screen's own error paragraph.
    const saved = await Promise.race([
      saving,
      page
        .locator('.line.warn')
        .waitFor({ state: 'visible', timeout: testInfo.timeout })
        .then(async () => {
          throw new Error(`the run failed: ${await page.locator('.line.warn').innerText()}`);
        }),
    ]);

    await expect(page.locator('.done')).toBeVisible();
    // The shipped path. A browser that buffered the cells or the map instead would be measuring a
    // mode the estimate was not taken for.
    await expect(page.locator('.done')).toContainText(/cells streamed\s*·\s*map disk/);

    // The engine verifies the sealed map by reading it back, and refuses the run when it disagrees.
    // Reaching `.done` with no error is that verdict; nothing else here re-checks the bytes.
    const path = (await saved.path())!;
    notes.push(`map: ${saved.suggestedFilename()}, ${(await stat(path)).size} B, sha256 ${await sha256(path)}`);
    await saved.delete();

    const { wasmMemoryBytes } = await profile;
    notes.push(
      `wasm linear memory: ${wasmMemoryBytes} B, ` +
        `${(wasmMemoryBytes / estimate.engineBytes).toFixed(3)} x the engine term`,
    );
    expect(wasmMemoryBytes).toBeGreaterThan(0);
    expect(wasmMemoryBytes, 'the assembly held more memory than its estimate priced').toBeLessThanOrEqual(
      estimate.engineBytes * MARGIN,
    );
    expect(errors).toEqual([]);
  } catch (error) {
    failure = error;
    await mkdir(testInfo.outputDir, { recursive: true });
    await page.screenshot({ path: testInfo.outputPath('failure.png'), fullPage: true }).catch(() => {});
  } finally {
    await context.close();
    // The profile holds the cells, the map and the spill — gigabytes that mean nothing outside
    // this run.
    await rm(profileDir, { recursive: true, force: true });
  }

  // The numbers are the point of the run, so they are recorded whether it passed or not.
  await mkdir(testInfo.outputDir, { recursive: true });
  const log = testInfo.outputPath('live-memory.log');
  await writeFile(log, notes.join('\n'));
  await testInfo.attach('live-memory', { path: log, contentType: 'text/plain' });
  if (failure) throw failure;
});
