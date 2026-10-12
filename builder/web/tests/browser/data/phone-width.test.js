import { readFileSync } from 'node:fs';
import { expect, test } from '@playwright/test';

const setup = JSON.parse(readFileSync(new URL('../../../test-support/data/setup.json', import.meta.url), 'utf8'));
const SCREENS = ['#/', '#/prod', '#/local', '#/sources', '#/runs', '#/machine'];

test.use({ viewport: { width: 390, height: 844 } });

test('every screen fits a phone with no horizontal page scroll', async ({ page }) => {
  await page.route('**/api/cli', route => route.fulfill({ json: { exit: 0, output: setup } }));
  await page.goto('/data.html');
  await expect(page.getByText('This page runs on')).toBeVisible();
  for (const hash of SCREENS) {
    await page.evaluate(next => { location.hash = next; }, hash);
    await expect(page.locator('nav a[aria-current="page"]')).toHaveAttribute('href', hash);
    expect(await page.evaluate(() => document.documentElement.scrollWidth), hash).toBeLessThanOrEqual(390);
  }
  await expect(page.getByRole('heading', { name: 'What this machine can do' })).toBeVisible();
});
