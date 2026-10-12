import { readFileSync } from 'node:fs';
import { expect, test } from '@playwright/test';

const fixture = name => JSON.parse(readFileSync(new URL(`../../../test-support/data/${name}.json`, import.meta.url), 'utf8'));
const [status, running, failed, review] = [fixture('status'), fixture('run-running'), fixture('run-failed'), fixture('review')];
const elsewhere = status.targets.find(target => target.lock).lock.run;
const ANSWERS = {
  '[]': status,
  '["setup"]': fixture('setup'),
  '["runs"]': fixture('runs'),
  [JSON.stringify(['runs', running.id])]: running,
  [JSON.stringify(['runs', failed.id])]: failed,
  [JSON.stringify(['update', 'local', '--dry-run'])]: review,
  [JSON.stringify(['update', 'local', '--add', 'sun', '--dry-run'])]: review,
};
// Each screen, and a text that it shows once its answers came.
const SCREENS = [
  ['#/', 'Select a layer'],
  ['#/prod', 'Releases on prod'],
  ['#/local', 'Layers of freiburg'],
  ['#/local/review', `Review ${review.id}`],
  [`#/local/resume/${failed.id}`, 'This review resumes run'],
  ['#/sources', 'Not built yet'],
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
    await expect(page.locator('nav a[aria-current="page"]')).toHaveAttribute('href', hash.split('/').slice(0, 2).join('/'));
    await expect(page.getByText(shows).first()).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth), hash).toBeLessThanOrEqual(390);
  }
});
