// node quiet2.mjs stack|coarse|recommended|hover|video [STACK MINZOOM]: the pictures of the second quiet round (marks.json).
import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, readdirSync, renameSync, writeFileSync } from 'node:fs';
import { AREAS, launch, open, view } from './lib.mjs';
const OUT = '/tmp/claude-0/-home-user-OpenBikeComputer/2d1ce3ff-4225-5a70-a539-37426ae550b6/scratchpad/places-quiet2';
mkdirSync(`${OUT}/single`, { recursive: true });
mkdirSync(`${OUT}/work`, { recursive: true });
const PLACES = { town: AREAS.town, village: [8.5962, 46.6337] };
const [what, recStack = 'kinds', recFirst = '16'] = process.argv.slice(2);
const report = existsSync(`${OUT}/marks.json`) ? JSON.parse(readFileSync(`${OUT}/marks.json`)) : {};
const browser = await launch();

/** The marks in view: counts by look, and for the rows of icons the longest row, the spots with more than 3 icons and the icons that touch another. */
const measure = page => page.evaluate(() => {
    const items = window.__places.badges.info(), counts = {}, spots = new Map();
    for (const item of items) {
        counts[item.look] = (counts[item.look] ?? 0) + 1;
        if (item.spot) spots.set(item.spot, [...(spots.get(item.spot) ?? []), item]);
    }
    const icons = items.filter(item => item.look === 'glyph');
    const touching = icons.filter(a => icons.some(b => a !== b && Math.abs(a.x - b.x) < 14 && Math.abs(a.y - b.y) < 14)).length;
    const rows = [...spots.values()];
    const longest = rows.reduce((best, row) => (row.length > best.length ? row : best), []);
    return {
        counts, longest: longest.length, over3: rows.filter(row => row.length > 3).length, touching,
        busiest: longest.length ? { x: longest.reduce((s, i) => s + i.x, 0) / longest.length, y: longest.reduce((s, i) => s + i.y, 0) / longest.length } : null,
        items: items.map(({ look, x, y, count, spot }) => ({ look, x, y, count, spot })),
    };
});

async function shoot(prefix, extra, area, zooms, scheme = 'light') {
    const { page, context, log } = await open(browser, { mode: 'badges', extra, scheme });
    for (const zoom of zooms) {
        await view(page, PLACES[area] ?? AREAS[area], zoom);
        const file = `${prefix}-z${zoom}`;
        if (await page.getByText('Some map data could not load').count()) throw new Error(`${file}: map data did not load`);
        await page.locator('.map-area').screenshot({ path: `${OUT}/single/${file}.png` });
        report[file] = await measure(page);
        const { items, ...rest } = report[file];
        console.log(file, JSON.stringify(rest));
    }
    const bad = log.filter(line => !line.startsWith('places prototype'));
    if (bad.length) console.log(bad.slice(0, 5).join('\n'));
    await context.close();
}

if (what === 'stack') for (const area of (recStack === 'village' ? ['village'] : ['town', 'village'])) for (const stack of ['kinds', 'each', 'ring']) for (const first of [15, 16])
    await shoot(`stack-${area}-${stack}-m${first}`, `&quiet=glyph&minzoom=${first}&stack=${stack}`, area, [15, 16, 17]);
if (what === 'coarse') for (const [name, radius] of [['fine', 26], ['coarse', 64]])
    await shoot(`hl-${name}`, `&quiet=glyph&minzoom=16&highlight=water&dim=1&hlradius=${radius}`, 'town', [12, 13, 14]);
if (what === 'recommended') for (const area of ['town', 'village']) {
    await shoot(`rec-${area}-water`, `&quiet=glyph&minzoom=${recFirst}&stack=${recStack}&highlight=water&dim=1`, area, [13, 15, 16]);
    await shoot(`rec-${area}-none`, `&quiet=glyph&minzoom=${recFirst}&stack=${recStack}`, area, [13, 15, 16]);
}

if (what === 'hover') {
    // Rest, hover and pressed of a single quiet icon, a row of icons and a rust disc, at 4x, light and dark; and keyboard focus.
    for (const scheme of ['light', 'dark']) {
        const { page, context } = await open(browser, { mode: 'badges', extra: '&quiet=glyph&minzoom=15&stack=kinds&highlight=water', scheme, scale: 4 });
        await view(page, AREAS.town, 15.5);
        const { items } = await measure(page);
        const box = await page.locator('.map-area').boundingBox();
        const inside = item => item.x > 60 && item.x < 820 && item.y > 60 && item.y < 600;
        const near = (list, avoid) => list.filter(inside).find(item => !avoid.some(other => other !== item && Math.hypot(other.x - item.x, other.y - item.y) < 30 && !(item.spot && other.spot === item.spot)));
        const spots = new Map();
        for (const item of items) if (item.spot) spots.set(item.spot, [...(spots.get(item.spot) ?? []), item]);
        const row = [...spots.values()].filter(row => row.length >= 2 && row.every(inside)).sort((a, b) => b.length - a.length)[0];
        const targets = { icon: near(items.filter(item => item.look === 'glyph' && !item.spot), items), row: row[row.length - 1], disc: near(items.filter(item => item.look === 'hl'), items) };
        const center = { icon: targets.icon, row: { x: row.reduce((s, i) => s + i.x, 0) / row.length, y: row[0].y }, disc: targets.disc };
        for (const [name, target] of Object.entries(targets)) {
            const c = center[name], clip = { x: box.x + c.x - 40, y: box.y + c.y - 22, width: 80, height: 44 };
            await page.mouse.move(box.x + 5, box.y + 640);
            await page.waitForTimeout(300);
            await page.screenshot({ path: `${OUT}/work/hover-${name}-rest-${scheme}.png`, clip });
            await page.mouse.move(box.x + target.x, box.y + target.y);
            await page.waitForTimeout(300);
            await page.screenshot({ path: `${OUT}/work/hover-${name}-hover-${scheme}.png`, clip });
            await page.mouse.down();
            await page.waitForTimeout(300);
            await page.screenshot({ path: `${OUT}/work/hover-${name}-pressed-${scheme}.png`, clip });
            // Leave the mark before release, so that nothing opens.
            await page.mouse.move(box.x + 5, box.y + 640);
            await page.mouse.up();
            await page.keyboard.press('Escape');
            // The release away from the mark panned the map: back to the view.
            await view(page, AREAS.town, 15.5);
            await page.evaluate(([x, y]) => { const el = document.elementFromPoint(x, y)?.closest('.places-mark'); el?.focus({ focusVisible: true, preventScroll: true }); }, [box.x + target.x, box.y + target.y]);
            await page.waitForTimeout(300);
            await page.screenshot({ path: `${OUT}/work/hover-${name}-focus-${scheme}.png`, clip });
            await page.evaluate(() => document.activeElement?.blur());
            console.log('hover', name, scheme);
        }
        await context.close();
    }
}

if (what === 'video') {
    // The pointer passes over a row of icons, a single icon and a disc, at normal speed and 5 times slower.
    for (const slow of [1, 5]) {
        const dir = `${OUT}/work/video-${slow}`, t0 = Date.now();
        const { page, context } = await open(browser, { mode: 'badges', extra: `&quiet=glyph&minzoom=15&stack=kinds&highlight=water&slow=${slow}`, scale: 1, viewport: { width: 1270, height: 873 }, video: dir });
        await view(page, AREAS.town, 15.5);
        const { items } = await measure(page);
        const box = await page.locator('.map-area').boundingBox();
        const inside = item => item.x > 200 && item.x < 700 && item.y > 150 && item.y < 500;
        const spots = new Map();
        for (const item of items) if (item.spot) spots.set(item.spot, [...(spots.get(item.spot) ?? []), item]);
        const row = [...spots.values()].filter(row => row.length >= 2 && row.every(inside)).sort((a, b) => b.length - a.length)[0] ?? [];
        const stops = [...row, items.find(item => item.look === 'glyph' && !item.spot && inside(item)), items.find(item => item.look === 'hl' && inside(item))].filter(Boolean);
        const start = Math.max(0, (Date.now() - t0) / 1000 - 0.5);
        for (const stop of stops) {
            await page.mouse.move(box.x + stop.x, box.y + stop.y, { steps: 12 });
            await page.waitForTimeout(500 * slow);
        }
        await page.mouse.move(box.x + 20, box.y + 600, { steps: 12 });
        await page.waitForTimeout(400 * slow);
        const video = page.video();
        await context.close();
        const file = await video.path();
        const name = `${OUT}/hover-${slow === 1 ? 'normal' : 'slow5x'}`;
        execFileSync('ffmpeg', ['-y', '-loglevel', 'error', '-ss', String(start), '-i', file, '-c:v', 'libvpx', '-b:v', '2M', `${name}.webm`]);
        // A crop round the stops, as a GIF.
        const xs = stops.map(s => box.x + s.x), ys = stops.map(s => box.y + s.y);
        const x0 = Math.max(0, Math.min(...xs) - 120), y0 = Math.max(0, Math.min(...ys) - 80), w = Math.min(560, 1270 - x0), h = Math.min(320, 873 - y0);
        execFileSync('ffmpeg', ['-y', '-loglevel', 'error', '-i', `${name}.webm`, '-vf', `crop=${w}:${h}:${Math.round(x0)}:${Math.round(y0)},fps=20,split[a][b];[a]palettegen=max_colors=128[p];[b][p]paletteuse`, `${name}.gif`]);
        console.log('video', slow, stops.length, 'stops');
    }
}

writeFileSync(`${OUT}/marks.json`, JSON.stringify(report));
await browser.close();
