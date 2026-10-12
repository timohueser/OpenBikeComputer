import { readFileSync } from 'node:fs';
import { expect, test } from '@playwright/test';

const fixture = name => JSON.parse(readFileSync(new URL(`../../../test-support/data/${name}.json`, import.meta.url), 'utf8'));
const ANSWERS = { '[]': fixture('status'), '["setup"]': fixture('setup') };
const SCREENS = ['#/', '#/prod', '#/local', '#/sources', '#/runs', '#/machine'];

test.use({ viewport: { width: 390, height: 844 } });

test('every screen fits a phone with no horizontal page scroll', async ({ page }) => {
  await page.route('**/api/cli', route => {
    const output = ANSWERS[JSON.stringify(route.request().postDataJSON().args)];
    return output ? route.fulfill({ json: { exit: 0, output } }) : route.abort();
  });
  await page.goto('/data.html');
  await expect(page.getByText('This page runs on')).toBeVisible();
  for (const hash of SCREENS) {
    await page.evaluate(next => { location.hash = next; }, hash);
    await expect(page.locator('nav a[aria-current="page"]')).toHaveAttribute('href', hash);
    if (hash === '#/machine') await expect(page.getByRole('heading', { name: 'What this machine can do' })).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth), hash).toBeLessThanOrEqual(390);
  }
});
