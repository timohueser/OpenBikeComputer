import { expect, test } from '@playwright/test';
import { mkdir, writeFile } from 'node:fs/promises';

test('Save, browse a ride, reset for route upload, Save again, and reload', async ({ page }, testInfo) => {
  const diagnostics = [`browser: ${page.context().browser().version()}`];
  const errors = [];
  page.on('console', (message) => {
    diagnostics.push(`${message.type()}: ${message.text()}`);
    if (message.type() === 'error') errors.push(message.text());
  });
  page.on('pageerror', (error) => {
    diagnostics.push(`pageerror: ${error.stack}`);
    errors.push(error.message);
  });

  async function screen(name) {
    await page.waitForFunction((expected) => {
      const api = window.wasmBindings;
      if (api?.obc_demo_reset_status() === 'Failed') throw new Error('Demo reset failed');
      return api?.obc_demo_ready() && api.obc_demo_reset_status() === 'Ready'
        && api.obc_demo_state() === expected;
    }, name, { timeout: 12_000 });
    diagnostics.push(`reset: Ready; screen: ${name}`);
    expect(errors).toEqual([]);
  }

  async function renderedCanvas() {
    await expect(page.locator('#device_canvas')).toBeVisible();
    return page.locator('#device_canvas').evaluate((canvas) => {
      const pixels = canvas.getContext('2d').getImageData(0, 0, canvas.width, canvas.height).data;
      const colors = new Set();
      for (let i = 0; i < pixels.length; i += 4) {
        if (pixels[i + 3]) colors.add(`${pixels[i]},${pixels[i + 1]},${pixels[i + 2]}`);
      }
      return { width: canvas.width, height: canvas.height, colors: colors.size };
    });
  }

  async function ready() {
    await page.locator('#device_stage').scrollIntoViewIfNeeded();
    await expect(page.getByRole('tab', { name: /Ride log/ })).toBeEnabled();
    const canvas = await renderedCanvas();
    expect(canvas).toMatchObject({ width: 240, height: 320 });
    expect(canvas.colors).toBeGreaterThan(1);
    expect(errors).toEqual([]);
  }

  async function saveRide() {
    await page.getByRole('tab', { name: /Ride log/ }).click();
    await screen('Map');
    await screen('RideControl');
    await screen('Home');
  }

  try {
    await page.goto('/');
    await ready();
    await expect(page.getByRole('tab', { name: 'Roll out', exact: true })).toHaveAttribute('aria-selected', 'true');
    await saveRide();
    const homeCanvas = await page.locator('#device_canvas').evaluate((canvas) => canvas.toDataURL());

    const controls = page.getByRole('group', { name: 'Device controls' });
    await controls.getByRole('button', { name: 'Select', exact: true }).click();
    await screen('Menu');
    await controls.getByRole('button', { name: 'Down', exact: true }).click();
    await controls.getByRole('button', { name: 'Select', exact: true }).click();
    await screen('Rides');
    await controls.getByRole('button', { name: 'Select', exact: true }).click();
    await screen('RideDetail');
    await expect.poll(() => page.locator('#device_canvas').evaluate((canvas) => canvas.toDataURL()))
      .not.toBe(homeCanvas);
    expect((await renderedCanvas()).colors).toBeGreaterThan(1);

    await page.getByRole('tab', { name: /Load route/ }).click();
    await screen('Home');
    await screen('RouteReceived');
    await saveRide();

    await page.reload();
    await ready();
    await screen('Map');
    await screen('RideControl');
    await screen('Home');
    expect((await renderedCanvas()).colors).toBeGreaterThan(1);
  } finally {
    await mkdir(testInfo.outputDir, { recursive: true });
    const path = testInfo.outputPath('browser-diagnostics.log');
    await writeFile(path, diagnostics.join('\n'));
    await testInfo.attach('browser-diagnostics', {
      path,
      contentType: 'text/plain',
    });
  }
});
