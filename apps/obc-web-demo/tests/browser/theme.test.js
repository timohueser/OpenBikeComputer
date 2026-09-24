import { expect, test } from '@playwright/test';

async function expectTheme(page, theme) {
  await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
  await expect(page.getByRole('button', { name: 'Dark mode', exact: true }))
    .toHaveAttribute('aria-pressed', String(theme === 'dark'));
  // The clear map corner uses the map's real light/dark style sets.
  await expect.poll(() => page.locator('#device_canvas').evaluate((canvas) => {
    const pixels = canvas.getContext('2d').getImageData(0, 0, 40, 40).data;
    let total = 0;
    for (let i = 0; i < pixels.length; i += 4) total += pixels[i] + pixels[i + 1] + pixels[i + 2];
    return total / (40 * 40 * 3) < 128;
  })).toBe(theme === 'dark');
}

for (const theme of ['light', 'dark']) {
  test(`Page and device start in ${theme} mode and keep a manual choice`, async ({ page }) => {
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    await page.goto('/');
    await page.waitForFunction(() => window.__obcDemo?.ready());
    await expectTheme(page, theme);
    const other = theme === 'dark' ? 'light' : 'dark';
    await page.emulateMedia({ colorScheme: other });
    await expectTheme(page, other);
    await page.getByRole('button', { name: 'Dark mode', exact: true }).press('Enter');
    await expectTheme(page, theme);
    await page.reload();
    await page.waitForFunction(() => window.__obcDemo?.ready());
    await expectTheme(page, theme);
    await page.locator('#device_stage').press('q');
    await page.waitForFunction(() => window.wasmBindings.obc_demo_state() === 'QuickDrawer');
    await page.getByRole('button', { name: 'Dark mode', exact: true }).click();
    await expectTheme(page, other);
  });
}

test('Theme switching works when browser storage is blocked', async ({ page }) => {
  await page.addInitScript(() => {
    Object.defineProperty(window, 'localStorage', { get() { throw new Error('Storage blocked'); } });
  });
  await page.emulateMedia({ colorScheme: 'light', reducedMotion: 'reduce' });
  await page.goto('/');
  await page.waitForFunction(() => window.__obcDemo?.ready());
  await page.getByRole('button', { name: 'Dark mode', exact: true }).click();
  await expectTheme(page, 'dark');
});
