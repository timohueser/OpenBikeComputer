// node zoom.mjs [PREFIX]: the pictures of the first zoom, the kinds and the pinned kinds, and the marks in view of each.
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { AREAS, launch, open, view } from './lib.mjs';
const OUT = '/tmp/claude-0/-home-user-OpenBikeComputer/2d1ce3ff-4225-5a70-a539-37426ae550b6/scratchpad/places-zoom';
mkdirSync(`${OUT}/single`, { recursive: true });
const DEFAULT = 'default', A = 'water.shop.camp', B = 'water.shop.camp.bike.peak';
// [area, name, kinds, minzoom, pinned, zooms]
const runs = [];
for (const area of ['town', 'valley']) {
    for (const first of [12, 13, 14]) runs.push([area, `default-first${first}`, DEFAULT, first, '', [12, 13, 14, 15]]);
    runs.push([area, 'default-first14-pinned-water', DEFAULT, 14, 'water', [12, 13, 14, 15]]);
}
for (const area of ['town', 'mountains']) for (const [name, kinds] of [['all', 'all'], ['default', DEFAULT], ['setA', A], ['setB', B]])
    runs.push([area, `${name}-first12`, kinds, 12, '', [13, 14]]);
const only = process.argv[2] ?? '';
const report = only && existsSync(`${OUT}/marks.json`) ? JSON.parse(readFileSync(`${OUT}/marks.json`)) : {};
const done = new Set();
const browser = await launch();
for (const [area, name, kinds, first, pinned, zooms] of runs.filter(([area, name]) => `${area}-${name}`.startsWith(only))) {
    const { page, context, log } = await open(browser, { mode: 'badges', kinds, extra: `&badge=tile&minzoom=${first}${pinned ? `&pinned=${pinned}` : ''}` });
    for (const zoom of zooms) {
        await view(page, AREAS[area], zoom);
        const file = `${area}-${name}-z${zoom}`;
        if (done.has(file)) continue;
        if (await page.getByText('Some map data could not load').count()) throw new Error(`${file}: map data did not load`);
        done.add(file);
        await page.locator('.map-area').screenshot({ path: `${OUT}/single/${file}.png` });
        report[file] = await page.evaluate(() => { const items = window.__places.badges.info(); return { marks: items.length, places: items.reduce((sum, item) => sum + item.count, 0) }; });
        console.log(file, JSON.stringify(report[file]));
    }
    const bad = log.filter(line => !line.startsWith('places prototype'));
    if (bad.length) console.log(bad.slice(0, 5).join('\n'));
    await context.close();
}
writeFileSync(`${OUT}/marks.json`, JSON.stringify(report, null, 1));
await browser.close();
