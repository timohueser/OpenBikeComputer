import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: '.',
  testMatch: '*.test.js',
  timeout: 60_000,
  expect: { timeout: 12_000 },
  workers: 1,
  retries: 0,
  forbidOnly: true,
  outputDir: '../../../../.artifacts/web-demo/browser',
  reporter: [
    ['list'],
    ['junit', { outputFile: '../../../../.artifacts/web-demo/junit.xml' }],
  ],
  use: {
    browserName: 'chromium',
    baseURL: 'http://127.0.0.1:4178',
    viewport: { width: 1440, height: 1100 },
    reducedMotion: 'no-preference',
    actionTimeout: 12_000,
    navigationTimeout: 15_000,
    screenshot: 'only-on-failure',
    trace: 'retain-on-failure',
  },
  webServer: {
    command: 'python3 -m http.server 4178 --bind 127.0.0.1 --directory ../../../../docs/dist',
    url: 'http://127.0.0.1:4178',
    reuseExistingServer: false,
    timeout: 10_000,
    stdout: 'pipe',
    stderr: 'pipe',
  },
});
