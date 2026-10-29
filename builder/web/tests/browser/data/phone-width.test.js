import { readFileSync } from 'node:fs';
import { expect, test } from '@playwright/test';

const fixture = name => JSON.parse(readFileSync(new URL(`../../../test-support/data/${name}.json`, import.meta.url), 'utf8'));
const [status, running, failed, planned] = [fixture('status'), fixture('run-running'), fixture('run-failed'), fixture('run-planned')];
const elsewhere = status.targets.find(target => target.lock).lock.run;
const ANSWERS = {
  '[]': status,
  '["setup"]': fixture('setup'),
  '["runs"]': fixture('runs'),
  '["areas"]': fixture('areas'),
  '["sources"]': fixture('sources'),
  '["sources","--region","grimsel"]': fixture('sources-region'),
  '["router","prod"]': fixture('router'),
  '["deploy"]': fixture('deploy'),
  [JSON.stringify(['source', 'prepare', 'wiki', '--region', 'europe/dach', '--languages', 'en,de', '--dry-run'])]: fixture('prepare-review'),
  [JSON.stringify(['runs', running.id])]: running,
  [JSON.stringify(['runs', failed.id])]: failed,
  [JSON.stringify(['runs', planned.id])]: planned,
};
const WIKI = '#/sources/wiki/prepare?region=europe/dach';
// Each screen, and a text that it shows once its answers came.
const SCREENS = [
  ['#/', 'Country, region or saved area'],
  ['#/overview', 'Select a layer'],
  ['#/prod', 'Router · serves'],
  ['#/local', 'Serve local'],
  [`#/bake?plan=${planned.id}`, 'reuses 1 from earlier bakes'],
  ['#/sources', 'Map styles and tools'],
  ['#/sources/geofabrik-extracts', 'Licence and credit'],
  ['#/sources/forest', 'A run on laptop stopped without freeing the lock'],
  ['#/sources?by=layer', 'By region tells'],
  ['#/sources?by=region&region=grimsel', 'OSM per update'],
  [WIKI, 'When it finishes'],
  ['#/runs', 'The runs of this machine'],
  [`#/runs/${running.id}`, 'Layers in this run'],
  [`#/runs/${failed.id}`, 'The next update reuses them'],
  [`#/runs/${elsewhere}`, 'You can watch this run here'],
  ['#/machine', 'What this machine can do'],
];

test.use({ viewport: { width: 390, height: 844 } });

test('every screen fits a phone with no horizontal page scroll', async ({ page }) => {
  await page.route('**/api/cli', route => {
    const output = ANSWERS[JSON.stringify(route.request().postDataJSON().args)];
    return output ? route.fulfill({ json: { exit: 0, output } }) : route.abort();
  });
  await page.goto('/data.html');
  await expect(page.getByText('This page runs on')).toBeVisible();
  for (const [hash, shows] of SCREENS) {
    await page.evaluate(next => { location.hash = next; }, hash);
    if (hash === WIKI) {
      await page.getByLabel('French').uncheck();
      await page.getByLabel('Spanish').uncheck();
      expect(await page.evaluate(() => document.documentElement.scrollWidth), 'wiki options').toBeLessThanOrEqual(390);
      await page.getByRole('button', { name: 'Review preparation' }).click();
    }
    const tab = hash.startsWith('#/sources') ? '#/sources' : hash.split('/').slice(0, 2).join('/');
    await expect(page.locator('nav a[aria-current="page"]')).toHaveAttribute('href', tab);
    await expect(page.getByText(shows).first()).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth), hash).toBeLessThanOrEqual(390);
    if (hash === WIKI) {
      await expect(page.getByText(/· Cached/)).toBeVisible();
      await expect(page.getByText(/Start downloads missing archives/)).toBeVisible();
      await page.getByLabel('German').uncheck();
      await expect(page.getByRole('button', { name: 'Start', exact: true })).toHaveCount(0);
    }
  }
});
