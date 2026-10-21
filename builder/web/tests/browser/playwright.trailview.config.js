import { defineConfig } from '@playwright/test';
import { EXECUTABLE, SWIFTSHADER } from './trailview/harness.js';

// The shipped TrailView build, served from loopback. `pretest:trailview` points it at the fixture
// config, and the test answers its tile host in the page, so nothing leaves the machine.
const PORT = Number(process.env.OBC_TRAILVIEW_BROWSER_PORT || 4183);
if (!Number.isInteger(PORT) || PORT < 1 || PORT > 65535) {
  throw new Error('OBC_TRAILVIEW_BROWSER_PORT must be an integer from 1 to 65535');
}

export default defineConfig({
  testDir: 'trailview',
  testMatch: '*.test.js',
  // SwiftShader draws a frame in seconds.
  timeout: 180_000,
  workers: 1,
  retries: 0,
  forbidOnly: true,
  outputDir: '../../../../.artifacts/web-builder/trailview',
  reporter: [
    ['list'],
    ['junit', { outputFile: '../../../../.artifacts/web-builder/trailview-junit.xml' }],
  ],
  use: {
    browserName: 'chromium',
    baseURL: `http://127.0.0.1:${PORT}`,
    viewport: { width: 960, height: 600 },
    launchOptions: { executablePath: EXECUTABLE, args: SWIFTSHADER },
    actionTimeout: 15_000,
    navigationTimeout: 15_000,
    screenshot: 'only-on-failure',
    trace: 'retain-on-failure',
  },
  webServer: {
    command: `python3 -m http.server ${PORT} --bind 127.0.0.1 --directory ../../dist/trailview`,
    url: `http://127.0.0.1:${PORT}/trailview.html`,
    reuseExistingServer: false,
    timeout: 10_000,
  },
});
