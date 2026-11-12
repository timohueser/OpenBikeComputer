// PROTOTYPE for #3361, never merged: runs the view search of prototype-still-view.js in Chromium,
// cold (a new browser) and warm (the same browser again), and writes the views as jobs for
// prototype-still-cost.mjs.
//
//   node builder/web/tests/browser/trailview/prototype-still-angle.mjs --terrain URL --routes A.json,B.json --out DIR
//
// --terrain is the release's terrain template, `.../terrain/{z}/{x}/{y}.webp`.
import { readFileSync, writeFileSync } from 'node:fs';
import { basename, join } from 'node:path';
import { parseArgs } from 'node:util';
import { chromium } from 'playwright';
import { serve } from './harness.js';

const { values: a } = parseArgs({ options: { terrain: { type: 'string' }, routes: { type: 'string' }, out: { type: 'string' } } });
const { origin, close } = await serve(new URL('./', import.meta.url).pathname);
const jobs = [];
try {
    for (const path of a.routes.split(',')) {
        const name = basename(path, '.json'), line = JSON.parse(readFileSync(path, 'utf8'));
        const browser = await chromium.launch();
        const page = await browser.newPage();
        // Any page of this origin, so the module loads from the same origin.
        await page.goto(`${origin}/smoke.test.js`);
        let result;
        for (const run of ['cold', 'warm']) {
            result = await page.evaluate(async ([line, template]) => (await import('/prototype-still-view.js')).chooseView(line, template), [line, a.terrain]);
            console.log(JSON.stringify({ name, run, ...result }));
        }
        await browser.close();
        const v = result.view;
        jobs.push({ name: `${name}-b`, view: `?lat=${v.lat.toFixed(6)}&lon=${v.lon.toFixed(6)}&heading=${v.heading.toFixed(1)}&pitch=${v.pitch.toFixed(1)}&alt=${v.alt.toFixed(0)}&date=2026-08-15&time=15:00&weather=sunny&scale=0.75`, width: 1920, height: 1080, route: path });
    }
} finally {
    close();
}
writeFileSync(join(a.out, 'angle-jobs.json'), JSON.stringify(jobs, null, 1));
