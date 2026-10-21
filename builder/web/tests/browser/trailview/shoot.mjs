// Draws a list of views of the built TrailView page in headless Chromium and writes one PNG per view.
//
//   npm run pretest:trailview --prefix builder/web/tests/browser     # build against the fixture
//   node builder/web/tests/browser/trailview/shoot.mjs [--views FILE] [--out DIR] [--tiles FILE] [--ground DIR] [--colour V] [--lab LAB] [--hud] [--gpu]
//
// --views is a JSON list of {name, view}, where view is a query string (default: views.json).
// --tiles serves a `planner-dem` MBTiles instead of the fixture. --ground serves the TrailView bakes
// of a folder, --colour its colour variant (webp-12). --lab adds `lab=LAB` to each view.
// --gpu draws on the machine's GPU in a browser window, else SwiftShader draws on the CPU.
// Prints one JSON line per view: the file, the adapter, the nodes drawn, the requests, the red
// pixels of a `debug=cracks` view and page errors.

import { mkdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { parseArgs } from 'node:util';
import { errorsOf, launch, redPixels, routeFixture, serve } from './harness.js';

const { values } = parseArgs({
    options: {
        views: { type: 'string', default: new URL('./views.json', import.meta.url).pathname },
        out: { type: 'string', default: new URL('../../../../../.artifacts/trailview-shots', import.meta.url).pathname },
        tiles: { type: 'string' },
        ground: { type: 'string' },
        colour: { type: 'string' },
        lab: { type: 'string' },
        hud: { type: 'boolean' },
        gpu: { type: 'boolean' },
    },
});
const views = JSON.parse(readFileSync(values.views, 'utf8'));
mkdirSync(values.out, { recursive: true });
const { origin, close } = await serve();
const browser = await launch(values.gpu);
try {
    for (const { name, view } of views) {
        const page = await browser.newPage({ viewport: { width: 1280, height: 800 }, deviceScaleFactor: 1 });
        const errors = errorsOf(page);
        await routeFixture(page, values.tiles, values.ground, values.colour);
        await page.goto(`${origin}/trailview.html${values.lab ? `${view}&lab=${values.lab}` : view}`);
        await page.waitForFunction(() => window.trailView, null, { timeout: 60_000 });
        await page.evaluate(() => window.trailView.waitForIdle());
        if (!values.hud) await page.addStyleTag({ content: '#hud { display: none; }' });
        const file = join(values.out, `${name}.png`);
        await page.screenshot({ path: file });
        const stats = await page.evaluate(() => window.trailView.stats());
        const red = view.includes('debug=cracks') ? { redPixels: await redPixels(page) } : {};
        console.log(JSON.stringify({ name, file, adapter: stats.adapter, drawn: stats.drawn, requests: stats.requests, ...red, errors }));
        await page.close();
    }
} finally {
    await browser.close();
    close();
}
