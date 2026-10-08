import { defineConfig } from '@playwright/test';

const PORT = Number(process.env.OBC_BROWSER_PORT || 4178);
if (!Number.isInteger(PORT) || PORT < 1 || PORT > 65535) {
  throw new Error('OBC_BROWSER_PORT must be an integer from 1 to 65535');
}

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
    baseURL: `http://127.0.0.1:${PORT}`,
    viewport: { width: 1440, height: 1100 },
    reducedMotion: 'no-preference',
    actionTimeout: 12_000,
    navigationTimeout: 15_000,
    screenshot: 'only-on-failure',
    trace: 'retain-on-failure',
  },
  webServer: {
    command: `python3 -m http.server ${PORT} --bind 127.0.0.1 --directory ../../../../docs/dist`,
    url: `http://127.0.0.1:${PORT}`,
    reuseExistingServer: false,
    timeout: 10_000,
    stdout: 'pipe',
    stderr: 'pipe',
  },
});
