// Opens the built TrailView page in a browser window on the machine's GPU, with the tiles that
// flight.mjs serves, until the window closes. `box.sh view` passes the bake of box.sh.
//
//   node builder/web/tests/browser/trailview/view.mjs [--view QUERY] [--tiles FILE] [--ground DIR] [--colour V] [--features DIR]
//
// --view is a view as a query string (default: the first view of views.json).

import { readFileSync } from 'node:fs';
import { parseArgs } from 'node:util';
import { launch, routeFixture, serve } from './harness.js';

const views = JSON.parse(readFileSync(new URL('./views.json', import.meta.url), 'utf8'));
const { values } = parseArgs({
    options: {
        view: { type: 'string', default: views[0].view },
        tiles: { type: 'string' },
        ground: { type: 'string' },
        colour: { type: 'string' },
        features: { type: 'string' },
    },
});
const { origin, close } = await serve();
const browser = await launch(true, true);
const page = await browser.newPage({ viewport: null });
await routeFixture(page, values.tiles, values.ground, values.colour, values.features);
await page.goto(`${origin}/trailview.html${values.view}`);
await page.waitForEvent('close', { timeout: 0 });
await browser.close();
close();
