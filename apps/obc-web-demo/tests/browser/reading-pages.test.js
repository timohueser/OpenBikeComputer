import { expect, test } from '@playwright/test';

test('reading pages share the theme preference and fit a narrow phone', async ({ page }) => {
  await page.emulateMedia({ colorScheme: 'dark' });
  await page.setViewportSize({ width: 320, height: 760 });
  await page.goto('/docs/');
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  await page.getByRole('button', { name: 'Dark mode', exact: true }).click();
  for (const path of ['/blog/', '/blog/the-site-has-a-log-now/', '/docs/software/formats/', '/docs/impressum/', '/docs/datenschutz/']) {
    await page.goto(path);
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');
    await expect(page.getByRole('button', { name: 'Dark mode', exact: true })).toHaveAttribute('aria-pressed', 'false');
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(320);
  }
  await page.getByRole('button', { name: 'Dark mode', exact: true }).click();
  await page.goto('/');
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
});

test('mobile docs navigation closes with Escape and exposes section links', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto('/docs/software/formats/');
  const menu = page.getByRole('button', { name: 'Documentation chapters' });
  await expect(page.locator('#docs-navigation')).toHaveJSProperty('inert', true);
  await menu.click();
  await expect(menu).toHaveAttribute('aria-expanded', 'true');
  await expect(page.locator('.sidebar [aria-current="page"]')).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(menu).toBeFocused();
  await expect(page.locator('#docs-navigation')).toHaveJSProperty('inert', true);
  await page.locator('.toc-mobile summary').click();
  const section = page.locator('.toc-mobile a').first();
  const hash = await section.getAttribute('href');
  await section.click();
  await expect(page).toHaveURL(new RegExp(`${hash}$`));
  await expect(page.locator('.toc-mobile')).not.toHaveAttribute('open');
  await menu.click();
  await page.getByRole('navigation', { name: 'Documentation', exact: true }).getByRole('link', { name: 'Overview', exact: true }).click();
  await expect(page).toHaveURL(/\/docs\/$/);
});

test('blog images open from the keyboard and restore focus after closing', async ({ page }) => {
  await page.goto('/blog/the-site-has-a-log-now/');
  const image = page.getByRole('button', { name: /^Enlarge image:/ }).first();
  await image.focus();
  await page.keyboard.press('Enter');
  const dialog = page.getByRole('dialog', { name: 'Image preview' });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Close', exact: true })).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(dialog).not.toBeVisible();
  await expect(image).toBeFocused();
});

test('global navigation remains available on mobile and restores keyboard focus', async ({ page }) => {
  for (const [path, current] of [['/', null], ['/docs/', 'Docs'], ['/blog/', 'Blog']]) {
    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto(path);
    const menu = page.getByRole('button', { name: 'Menu', exact: true });
    const navigation = page.getByRole('navigation', { name: 'Main navigation', exact: true });
    await expect(navigation).toBeHidden();
    await menu.focus();
    await page.keyboard.press('Enter');
    await expect(menu).toHaveAttribute('aria-expanded', 'true');
    for (const destination of ['Docs', 'Blog', 'Maps', 'GitHub']) {
      await expect(navigation.getByRole('link', { name: destination, exact: true })).toBeVisible();
    }
    if (current) await expect(navigation.getByRole('link', { name: current, exact: true })).toHaveAttribute('aria-current', 'page');
    await page.keyboard.press('Tab');
    await page.keyboard.press('Escape');
    await expect(menu).toBeFocused();
    await expect(navigation).toBeHidden();
    await menu.click();
    await page.setViewportSize({ width: 1280, height: 900 });
    await expect(menu).toBeHidden();
    await expect(navigation).toBeVisible();
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(menu).toHaveAttribute('aria-expanded', 'false');
    await expect(navigation).toBeHidden();
  }
});

test('landing anchors stay below the sticky header', async ({ page }) => {
  for (const width of [390, 1280]) {
    await page.setViewportSize({ width, height: 900 });
    for (const anchor of ['demo', 'features']) {
      await page.goto(`/#${anchor}`);
      await expect.poll(() => page.locator(`#${anchor}`).evaluate(element =>
        element.getBoundingClientRect().top - document.querySelector('.site-head').getBoundingClientRect().bottom
      )).toBeGreaterThanOrEqual(-1);
    }
  }
});
