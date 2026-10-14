import { expect, test } from '@playwright/test';
import { CORS_PREFLIGHT, classify, respond } from './stubs.js';

for (const advanced of [false, true]) {
  test(`terrain capabilities require advertised, activity-specific selection (${advanced ? 17 : 15} profiles)`, async ({ page }) => {
    await page.route(url => url.hostname !== '127.0.0.1', route => {
      const request = route.request();
      if (request.method() === 'OPTIONS') return route.fulfill(CORS_PREFLIGHT);
      const response = respond(request);
      if (advanced && classify(new URL(request.url())) === 'region') response.json.profiles.push('mtb/technical', 'hiking/alpine');
      return response ? route.fulfill(response) : route.abort();
    });
    await page.goto('/planner.html');
    const activity = page.getByRole('combobox', { name: 'Activity' });
    const preset = page.getByRole('combobox', { name: 'Preset' });
    await expect(activity).toBeEnabled();
    for (const [bike, choice, warning] of [
      ['Hiking', 'Alpine (T4)', 'alpine experience required'],
      ['Mountain bike', 'Technical (S3)', 'Advanced riding skills required'],
    ]) {
      await activity.click();
      await page.getByRole('option', { name: bike, exact: true }).click();
      await expect(preset).toHaveText('Balanced');
      await preset.click();
      const option = page.getByRole('option').filter({ hasText: choice });
      if (advanced) {
        await expect(option).toContainText(warning);
        await option.click();
        await expect(preset).toHaveText(choice);
      } else {
        await expect(option).toHaveCount(0);
        await preset.press('Escape');
      }
    }
    await activity.click();
    await page.getByRole('option', { name: 'Road bike', exact: true }).click();
    await expect(preset).toHaveText('Balanced');
    await preset.click();
    await expect(page.getByRole('option')).toHaveText(['Balanced', 'Shorter', 'Less climbing']);
  });
}
