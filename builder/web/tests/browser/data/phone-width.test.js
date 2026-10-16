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
  '["router","prod"]': fixture('router'),
  [JSON.stringify(['source', 'prepare', 'wiki', '--to', 'prod', '--region', 'europe/dach', '--languages', 'en,de', '--dry-run'])]: fixture('prepare-review'),
  [JSON.stringify(['runs', running.id])]: running,
  [JSON.stringify(['runs', failed.id])]: failed,
  [JSON.stringify(['runs', planned.id])]: planned,
};
// Each screen, and a text that it shows once its answers came.
const SCREENS = [
  ['#/', 'Select a layer'],
  ['#/prod', 'Router · serves'],
  ['#/local', 'Serve local'],
  ['#/local/review', 'Choose what local serves'],
  [`#/local/review?plan=${planned.id}`, `Plan ${planned.review.id}`],
  [`#/local/resume/${failed.id}`, 'These settings resume run'],
  ['#/sources', 'Each external dataset'],
  ['#/sources/wiki', 'Prepared versions'],
  ['#/sources/terrain-reference', 'preparing on hetzner-bake-1'],
  ['#/sources/wiki/prepare?to=prod', 'When it finishes'],
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
    if (hash === '#/sources/wiki/prepare?to=prod') {
      await page.getByLabel('Geofabrik area').fill('europe/dach');
      await page.getByLabel('French').uncheck();
      await page.getByLabel('Spanish').uncheck();
      expect(await page.evaluate(() => document.documentElement.scrollWidth), 'wiki options').toBeLessThanOrEqual(390);
      await page.getByRole('button', { name: 'Review preparation' }).click();
    }
    await expect(page.locator('nav a[aria-current="page"]')).toHaveAttribute('href', hash.split('/').slice(0, 2).join('/'));
    await expect(page.getByText(shows).first()).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth), hash).toBeLessThanOrEqual(390);
    if (hash === '#/sources/wiki/prepare?to=prod') {
      await expect(page.getByText(/· Cached/)).toBeVisible();
      await expect(page.getByText(/Start downloads missing archives/)).toBeVisible();
      await page.getByLabel('German').uncheck();
      await expect(page.getByRole('button', { name: 'Start', exact: true })).toHaveCount(0);
    }
  }
});
