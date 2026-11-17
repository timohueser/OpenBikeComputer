// node quiet.mjs [PREFIX]: the pictures of the quiet looks and the highlight, and the marks in view of each (marks.json).
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { AREAS, launch, open, view } from './lib.mjs';
const OUT = '/tmp/claude-0/-home-user-OpenBikeComputer/2d1ce3ff-4225-5a70-a539-37426ae550b6/scratchpad/places-quiet';
mkdirSync(`${OUT}/single`, { recursive: true });
const LOOKS = { today: ['today', ''], badges: ['badges', '&badge=tile'], glyph: ['badges', '&quiet=glyph&minzoom=14'], dot: ['badges', '&quiet=dot&minzoom=14'], none: ['badges', '&quiet=none'] };
// [file prefix, mode, extra, area, zooms, scheme]
const runs = [];
for (const [area, zooms] of [['town', [14, 15]], ['valley', [13, 14]]]) for (const [look, [mode, extra]] of Object.entries(LOOKS)) runs.push([`looks-${area}-${look}`, mode, extra, area, zooms]);
for (const first of [14, 15]) runs.push([`glyph-m${first}-town`, 'badges', `&quiet=glyph&minzoom=${first}`, 'town', [13, 14, 15, 16]]);
for (const [kind, area, zooms] of [['water', 'town', [12, 13, 14, 15]], ['camp', 'valley', [12, 13, 14]], ['camp', 'mountains', [12, 13, 14]]])
    for (const dim of ['', '&dim=1']) runs.push([`hl${kind}${dim ? '-dim' : ''}-${area}`, 'badges', `&quiet=glyph&minzoom=14&highlight=${kind}${dim}`, area, zooms]);
runs.push(['dark-town', 'badges', '&quiet=glyph&minzoom=14&highlight=water', 'town', [14], 'dark']);
runs.push(['dark-dim-town', 'badges', '&quiet=glyph&minzoom=14&highlight=water&dim=1', 'town', [14], 'dark']);
runs.push(['dark-dot-town', 'badges', '&quiet=dot&minzoom=14&highlight=water', 'town', [14], 'dark']);

const only = process.argv[2] ?? '';
const report = existsSync(`${OUT}/marks.json`) ? JSON.parse(readFileSync(`${OUT}/marks.json`)) : {};
const browser = await launch();
for (const [prefix, mode, extra, area, zooms, scheme = 'light'] of runs.filter(([prefix]) => prefix.startsWith(only))) {
    const { page, context, log } = await open(browser, { mode, extra, scheme });
    for (const zoom of zooms) {
        await view(page, AREAS[area], zoom);
        const file = `${prefix}-z${zoom}`;
        if (await page.getByText('Some map data could not load').count()) throw new Error(`${file}: map data did not load`);
        await page.locator('.map-area').screenshot({ path: `${OUT}/single/${file}.png` });
        report[file] = await page.evaluate(() => {
            const { badges, map } = window.__places;
            // today: the icons of the map layer of develop.
            if (!badges) return { counts: { icons: map.queryRenderedFeatures({ layers: ['planner-poi-icons'] }).length }, items: [] };
            const items = badges.info(), counts = {};
            for (const item of items) counts[item.look] = (counts[item.look] ?? 0) + 1;
            return { counts, items: items.map(({ look, x, y, count }) => ({ look, x, y, count })) };
        });
        console.log(file, JSON.stringify(report[file].counts));
    }
    const bad = log.filter(line => !line.startsWith('places prototype'));
    if (bad.length) console.log(bad.slice(0, 5).join('\n'));
    await context.close();
}
writeFileSync(`${OUT}/marks.json`, JSON.stringify(report));
await browser.close();
