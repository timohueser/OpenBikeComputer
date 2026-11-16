// node shoot.mjs AREA[,AREA] MODE[,MODE] ZOOM[,ZOOM] [kinds] [suffix] [extra query]
import { writeFileSync } from 'node:fs';
import { AREAS, OUT, launch, open, view, shot } from './lib.mjs';
const [areas, modes, zooms, kinds = 'default', suffix = '', extra = ''] = process.argv.slice(2);
const browser = await launch();
const report = {};
for (const area of areas.split(',')) for (const mode of modes.split(',')) {
    const { page, context, log } = await open(browser, { mode, kinds, extra });
    for (const zoom of zooms.split(',').map(Number)) {
        const start = Date.now();
        await view(page, AREAS[area], zoom);
        const name = `${area}-z${zoom}-${mode}${kinds === 'all' ? '-all' : ''}${suffix}`;
        await shot(page, name);
        const info = await page.evaluate(() => ({ drawn: window.__places.layers.length ? window.__places.drawn().length : null, tiles: window.__places.loadedTiles.length, stats: window.__places.stats(), timings: window.__places.timings }));
        report[name] = info;
        console.log(name, `${Date.now() - start} ms`, 'drawn', info.drawn, 'tiles', info.tiles, 'places', info.stats.total);
    }
    if (log.some(line => !line.startsWith('places prototype'))) console.log(log.filter(line => !line.startsWith('places prototype')).slice(0, 5).join('\n'));
    await context.close();
}
writeFileSync(`${OUT}/report-${areas}-${modes}-${kinds}${suffix}.json`, JSON.stringify(report, null, 1));
await browser.close();
