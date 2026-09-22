import { defineConfig } from '@playwright/test';

// The memory gate's own run. Loopback serves only the shipped `dist/web`; the catalogue and its
// objects come from the live CDN, which is why this is a `live` suite.
const PORT = Number(process.env.OBC_BROWSER_PORT || 4182);
if (!Number.isInteger(PORT) || PORT < 1 || PORT > 65535) {
  throw new Error('OBC_BROWSER_PORT must be an integer from 1 to 65535');
}

export default defineConfig({
  testDir: '.',
  testMatch: 'live-memory.test.ts',
  // Downloads about 890 MB, assembles it and reads the map back. Generous, because a timeout here
  // is a failure, never a retry.
  timeout: 30 * 60_000,
  expect: { timeout: 60_000 },
  workers: 1,
  retries: 0,
  forbidOnly: true,
  outputDir: '../../../../.artifacts/web-builder-live/browser',
  reporter: [
    ['list'],
    ['junit', { outputFile: '../../../../.artifacts/web-builder-live/junit.xml' }],
  ],
  // The test launches its own persistent context (see its header), so these are the settings it
  // reads rather than fixture options the runner applies.
  use: {
    // The CDN's `Access-Control-Allow-Origin` names the deployed origins, and this page is on
    // loopback. The suite measures memory, not the same-origin policy.
    launchOptions: { args: ['--disable-web-security'] },
    baseURL: `http://127.0.0.1:${PORT}`,
    viewport: { width: 1440, height: 1100 },
    actionTimeout: 30_000,
  },
  webServer: {
    command: `python3 -m http.server ${PORT} --bind 127.0.0.1 --directory ../../dist/web`,
    url: `http://127.0.0.1:${PORT}`,
    reuseExistingServer: false,
    timeout: 10_000,
    stdout: 'pipe',
    stderr: 'pipe',
  },
});
