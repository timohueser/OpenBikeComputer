import { defineConfig } from '@playwright/test';

// The shipped planner build, served from loopback. `pretest:planner` points it at three hosts that
// do not exist, and the journey answers them in the page, so nothing leaves the machine.
const PORT = Number(process.env.OBC_PLANNER_BROWSER_PORT || 4181);
if (!Number.isInteger(PORT) || PORT < 1 || PORT > 65535) {
  throw new Error('OBC_PLANNER_BROWSER_PORT must be an integer from 1 to 65535');
}

export default defineConfig({
  testDir: 'planner',
  testMatch: '*.test.js',
  timeout: 180_000,
  workers: 1,
  retries: 0,
  forbidOnly: true,
  outputDir: '../../../../.artifacts/web-builder/planner',
  reporter: [
    ['list'],
    ['junit', { outputFile: '../../../../.artifacts/web-builder/planner-junit.xml' }],
  ],
  use: {
    browserName: 'chromium',
    baseURL: `http://127.0.0.1:${PORT}`,
    viewport: { width: 1440, height: 1100 },
    actionTimeout: 15_000,
    navigationTimeout: 15_000,
    screenshot: 'only-on-failure',
    trace: 'retain-on-failure',
  },
  webServer: {
    command: `python3 -m http.server ${PORT} --bind 127.0.0.1 --directory ../../dist/planner`,
    url: `http://127.0.0.1:${PORT}/planner.html`,
    reuseExistingServer: false,
    timeout: 10_000,
  },
});
