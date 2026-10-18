// Renders baked spots without a browser window: starts its own dev server, opens the page in
// headless Chromium, and saves the photo side by side and views through the page's own buttons.
//
//   npm run shoot -- <spot>[,<spot>…] [--yaw 0,90,180,270] [--pitch 0] [--vfov 60] [--hour 15]
//        [--season summer] [--weather sunny] [--no-photo] [--time] [--out dir] [--swiftshader]
//
// Shots land in ~/.cache/obc/trail-view/shots/ and, with --out, are copied to <dir>/. Prints one
// JSON line per spot: the files, the WebGL renderer, near and total triangles, page errors, and
// with --time the median frame time. Without a GPU, pass --swiftshader: WebGL2 then runs on the
// CPU, the same picture at about 2 s a frame, so its frame times say nothing about the product.

import { copyFileSync, mkdirSync, mkdtempSync } from 'node:fs';
import { homedir, tmpdir } from 'node:os';
import { join } from 'node:path';
import { parseArgs } from 'node:util';
import { chromium } from 'playwright';
import { createServer } from 'vite';

const ROOT = join(import.meta.dirname, '..');
const SHOTS = join(homedir(), '.cache/obc/trail-view/shots');

const { values, positionals } = parseArgs({
  allowPositionals: true,
  options: {
    yaw: { type: 'string' }, pitch: { type: 'string', default: '0' }, vfov: { type: 'string', default: '60' },
    hour: { type: 'string', default: '15' }, season: { type: 'string', default: 'summer' },
    weather: { type: 'string', default: 'sunny' }, 'no-photo': { type: 'boolean' }, time: { type: 'boolean' },
    out: { type: 'string' }, swiftshader: { type: 'boolean' },
  },
});
if (!positionals.length) {
  console.error('usage: npm run shoot -- <spot>[,<spot>…] [options]; see the head of tune/shoot.mjs');
  process.exit(2);
}

const browser = await chromium.launch({
  args: values.swiftshader ? ['--use-angle=swiftshader', '--enable-unsafe-swiftshader'] : ['--ignore-gpu-blocklist', '--enable-gpu'],
});
// Each run has its own server and dependency cache, so parallel agents do not share either.
const server = await createServer({
  root: ROOT, configFile: join(ROOT, 'vite.config.ts'), cacheDir: mkdtempSync(join(tmpdir(), 'trail-view-vite-')),
  server: { port: 0, strictPort: false, hmr: false, watch: null }, logLevel: 'error',
});
try {
  const page = await browser.newPage({ viewport: { width: 1400, height: 860 }, deviceScaleFactor: 1.5 });
  const renderer = await page.evaluate(() => {
    const gl = document.createElement('canvas').getContext('webgl2');
    const info = gl?.getExtension('WEBGL_debug_renderer_info');
    return gl ? gl.getParameter(info ? info.UNMASKED_RENDERER_WEBGL : gl.RENDERER) : null;
  });
  if (!renderer) throw new Error('no WebGL2 in headless Chromium: rerun with --swiftshader');
  await server.listen();
  // three.js reports shader compile errors on the console only.
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  page.on('console', (message) => message.type() === 'error' && errors.push(message.text()));
  // The page opens on the first spot, so it does not load another one first.
  await page.goto(`${server.resolvedUrls.local[0]}#spot=${positionals[0].split(',')[0]}`);
  await page.waitForFunction(() => window.trailView, null, { timeout: 60_000 });
  for (const spot of positionals.flatMap((p) => p.split(','))) {
    const result = await page.evaluate(async (o) => {
      const tv = window.trailView;
      const frames = (n) => new Promise((resolve) => { const step = () => (n-- ? requestAnimationFrame(step) : resolve()); step(); });
      Object.assign(tv.state, { preset: o.spot, hour: o.hour, weather: o.weather, season: tv.seasons.find((s) => s.startsWith(o.season)) ?? tv.state.season });
      await tv.select(o.spot);
      if (tv.loaded()?.bake.meta.id !== o.spot) throw new Error(`${o.spot} did not load`);
      tv.lightChanged();
      tv.gui.controllersRecursive().forEach((c) => c.updateDisplay());
      await frames(3);
      const files = [];
      if (o.photo && tv.loaded().bake.meta.photo) files.push(await tv.saveSideBySide());
      tv.state.photoCamera = false;
      for (const yaw of o.yaws ?? [tv.loaded().bake.meta.heading ?? 0]) {
        Object.assign(tv.state, { yaw, pitch: o.pitch, vfov: o.vfov });
        tv.layout();
        await frames(2);
        files.push(await tv.saveView());
      }
      return { files, triangles: tv.gui.controllersRecursive().find((c) => c.property === 'triangles').object.triangles };
    }, {
      spot, hour: Number(values.hour), season: values.season, weather: values.weather, photo: !values['no-photo'],
      yaws: values.yaw?.split(',').map(Number), pitch: Number(values.pitch), vfov: Number(values.vfov),
    });
    // Synchronous renders, each finished by a one-pixel readback, so the time includes the GPU's.
    const frameMs = values.time ? await page.evaluate(() => {
      const gl = [...document.querySelectorAll('canvas')].map((c) => c.getContext('webgl2')).find(Boolean);
      const pixel = new Uint8Array(4);
      const times = [];
      for (let i = 0; i < 45; i++) {
        const t0 = performance.now();
        window.trailView.render();
        gl.readPixels(0, 0, 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, pixel);
        if (i >= 5) times.push(performance.now() - t0);
      }
      return times.sort((a, b) => a - b)[times.length >> 1];
    }) : undefined;
    const files = result.files.filter(Boolean).map((name) => {
      if (!values.out) return join(SHOTS, name);
      mkdirSync(values.out, { recursive: true });
      copyFileSync(join(SHOTS, name), join(values.out, name));
      return join(values.out, name);
    });
    console.log(JSON.stringify({ spot, files, renderer, triangles: result.triangles, frameMs: frameMs && Math.round(frameMs * 10) / 10, errors: errors.splice(0) }));
  }
} finally {
  await browser.close();
  await server.close();
}
