// Draws a list of views of the built TrailView page in headless Chromium and writes one PNG per view.
//
//   npm run pretest:trailview --prefix builder/web/tests/browser     # build against the fixture
//   node builder/web/tests/browser/trailview/shoot.mjs [--views FILE] [--out DIR] [--width PX] [--height PX] [--tiles FILE] [--ground DIR] [--colour V] [--features DIR] [--hud] [--gpu] [--audit] [--pages N]
//
// --views is a JSON list of {name, view}, where view is a query string (default: views.json).
// --width and --height are the size of the page (default 1280 × 800).
// --tiles serves a `planner-dem` MBTiles instead of the fixture. --ground serves the TrailView bakes
// of a folder, --colour its colour variant (webp-12), --features a folder of feature tiles `z/x/y.bin`.
// --audit prints the invariant audit of each view.
// --gpu draws on the machine's GPU, headless on Linux and in a window on macOS, else SwiftShader
// draws on the CPU. Prints one JSON line per view: the file, the adapter, the nodes drawn, the
// requests, the red pixels of a `debug=cracks` view and page errors.
// --pages is the number of views that draw at once, each on its own page (default 3 with --gpu, else
// 1). The JSON lines come in the order in which the views finish.

import { mkdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { parseArgs } from 'node:util';
import { errorsOf, launch, redPixels, routeFixture, serve } from './harness.js';

const { values } = parseArgs({
    options: {
        views: { type: 'string', default: new URL('./views.json', import.meta.url).pathname },
        out: { type: 'string', default: new URL('../../../../../.artifacts/trailview-shots', import.meta.url).pathname },
        width: { type: 'string', default: '1280' },
        height: { type: 'string', default: '800' },
        tiles: { type: 'string' },
        ground: { type: 'string' },
        colour: { type: 'string' },
        features: { type: 'string' },
        hud: { type: 'boolean' },
        audit: { type: 'boolean' },
        gpu: { type: 'boolean' },
        pages: { type: 'string' },
    },
});
/** The longest wait for the page and for an idle view. */
const TIMEOUT_MS = 120_000;
const pages = Number(values.pages ?? (values.gpu ? 3 : 1));
if (!Number.isInteger(pages) || pages < 1) throw new Error(`--pages ${values.pages}: not a positive whole number`);
const views = JSON.parse(readFileSync(values.views, 'utf8'));
mkdirSync(values.out, { recursive: true });
const { origin, close } = await serve();
const browser = await launch(values.gpu);
/** Resolves after `waitForIdle()`; rejects after TIMEOUT_MS. */
const idle = page => page.evaluate(ms => Promise.race([
    window.trailView.waitForIdle(),
    new Promise((_, reject) => setTimeout(() => reject(new Error(`not idle after ${ms} ms`)), ms)),
]), TIMEOUT_MS);
async function shoot({ name, view }) {
    const page = await browser.newPage({ viewport: { width: Number(values.width), height: Number(values.height) }, deviceScaleFactor: 1 });
    const errors = errorsOf(page);
    await routeFixture(page, values.tiles, values.ground, values.colour, values.features);
    await page.goto(`${origin}/trailview.html${view}`);
    await page.waitForFunction(() => window.trailView, null, { timeout: TIMEOUT_MS });
    await idle(page);
    await page.evaluate(() => window.trailView.showMarker(null));
    if (!values.hud) await page.addStyleTag({ content: '#hud { display: none; }' });
    // The cleared marker changes the still view: it takes its jittered frames again.
    await idle(page);
    const file = join(values.out, `${name}.png`);
    await page.screenshot({ path: file });
    const stats = await page.evaluate(() => window.trailView.stats());
    const red = view.includes('debug=cracks') ? { redPixels: await redPixels(page) } : {};
    const audit = values.audit ? { audit: await page.evaluate(() => window.trailView.audit()) } : {};
    console.log(JSON.stringify({ name, file, adapter: stats.adapter, drawn: stats.drawn, requests: stats.requests, ...red, ...audit, errors }));
    await page.close();
}
try {
    let next = 0;
    await Promise.all(Array.from({ length: pages }, async () => {
        while (next < views.length) {
            const view = views[next++];
            await shoot(view).catch(error => { throw new Error(`${view.name}: ${error.message}`, { cause: error }); });
        }
    }));
} finally {
    await browser.close();
    close();
}
