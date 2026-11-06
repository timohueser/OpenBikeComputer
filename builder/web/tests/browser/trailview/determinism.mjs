// Draws each view several times and compares the runs: the hashes of the drawn terrain (engine.hashes:
// each drawn page's heights `h/` and mask `m/`, each records block `r/`, each piece's corners `p/`) and the
// pixels of the shots. A deterministic view gives one hash per region and equal shots. A debug tool, not a test.
//
//   npm run pretest:trailview --prefix builder/web/tests/browser     # build against the fixture
//   node builder/web/tests/browser/trailview/determinism.mjs --views FILE --out DIR [--runs N] [--pages N] [--query Q] [--gpu]
//
// --runs is the runs of each view (default 8), --pages the runs that draw at once, each on its own page
// (default 1); the runs of the views interleave. A run waits for the idle view, then until the records and
// the pieces stop changing, and reads the hashes twice, a second apart: a region that changes between the
// reads counts as unsettled. Writes DIR/<view>/<run>.png, DIR/hashes.json and DIR/report.md, and prints the report.

import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { parseArgs } from 'node:util';
import { errorsOf, launch, routeFixture, serve } from './harness.js';

const { values } = parseArgs({
    options: {
        views: { type: 'string' },
        out: { type: 'string' },
        runs: { type: 'string', default: '8' },
        pages: { type: 'string', default: '1' },
        query: { type: 'string' },
        gpu: { type: 'boolean' },
    },
});
const TIMEOUT_MS = 120_000;
const runs = Number(values.runs), pages = Number(values.pages);
const views = JSON.parse(readFileSync(values.views, 'utf8'));
mkdirSync(values.out, { recursive: true });
const { origin, close } = await serve();
const browser = await launch(values.gpu);
const idle = page => page.evaluate(ms => Promise.race([
    window.trailView.waitForIdle(),
    new Promise((_, reject) => setTimeout(() => reject(new Error(`not idle after ${ms} ms`)), ms)),
]), TIMEOUT_MS);
const snap = page => page.evaluate(() => {
    const s = window.trailView.stats(), p = s.terrain?.pieces, r = s.terrain?.records;
    return { key: JSON.stringify([p?.cells, p?.drawn, p?.triangles, r?.cells, r?.building]), building: r?.building ?? 0, records: r && { cells: r.cells, restarts: r.restarts, error: r.error } };
});

async function run({ name, view }, k) {
    const page = await browser.newPage({ viewport: { width: 1920, height: 960 }, deviceScaleFactor: 1 });
    const errors = errorsOf(page);
    await routeFixture(page);
    await page.goto(`${origin}/trailview.html${view}${values.query ? `&${values.query}` : ''}`);
    await page.waitForFunction(() => window.trailView, null, { timeout: TIMEOUT_MS });
    await idle(page);
    await page.evaluate(() => window.trailView.showMarker(null));
    await page.addStyleTag({ content: '#hud { display: none; }' });
    await idle(page);
    let prev = await snap(page);
    for (let round = 0; round < 8; round++) {
        await page.waitForTimeout(1000);
        await idle(page);
        const now = await snap(page);
        if (now.key === prev.key && now.building === 0) break;
        prev = now;
    }
    const first = await page.evaluate(() => window.trailView.hashes());
    await page.waitForTimeout(1000);
    await idle(page);
    const hashes = await page.evaluate(() => window.trailView.hashes());
    const unsettled = Object.keys(hashes).filter(region => first[region] !== undefined && first[region] !== hashes[region]);
    mkdirSync(join(values.out, name), { recursive: true });
    const file = join(values.out, name, `${k}.png`);
    await page.screenshot({ path: file });
    await page.close();
    console.log(JSON.stringify({ name, run: k, regions: Object.keys(hashes).length, unsettled: unsettled.length, records: prev.records, errors }));
    return { name, run: k, file, hashes, unsettled, records: prev.records, errors };
}

/** Per later run, the pixels that differ from the first run's and those by more than 8 in a channel; `diff` gets a PNG of the run that differs most: the first run dimmed, magenta where it differs by more than 8. */
async function pixelDiffs(files, diff) {
    const page = await browser.newPage();
    const out = await page.evaluate(async datas => {
        const load = async data => {
            const img = new Image();
            img.src = `data:image/png;base64,${data}`;
            await img.decode();
            const c = new OffscreenCanvas(img.width, img.height), g = c.getContext('2d');
            g.drawImage(img, 0, 0);
            return [g.getImageData(0, 0, img.width, img.height).data, img.width, img.height];
        };
        const [base, width, height] = await load(datas[0]), counts = [];
        let worst = { big: 0 }, picture;
        for (const data of datas.slice(1)) {
            const [other] = await load(data);
            let any = 0, big = 0;
            const marks = new Uint8ClampedArray(base.length);
            for (let i = 0; i < base.length; i += 4) {
                const d = Math.max(Math.abs(base[i] - other[i]), Math.abs(base[i + 1] - other[i + 1]), Math.abs(base[i + 2] - other[i + 2]));
                if (d > 0) any++;
                if (d > 8) big++;
                marks.set(d > 8 ? [255, 0, 255, 255] : [base[i] / 3, base[i + 1] / 3, base[i + 2] / 3, 255], i);
            }
            counts.push([any, big]);
            if (big > worst.big) worst = { big, marks };
        }
        if (worst.marks) {
            const c = new OffscreenCanvas(width, height);
            c.getContext('2d').putImageData(new ImageData(worst.marks, width, height), 0, 0);
            const bytes = new Uint8Array(await (await c.convertToBlob({ type: 'image/png' })).arrayBuffer());
            picture = btoa(Array.from(bytes, b => String.fromCharCode(b)).join(''));
        }
        return { counts, picture };
    }, files.map(file => readFileSync(file).toString('base64')));
    await page.close();
    if (out.picture) writeFileSync(diff, Buffer.from(out.picture, 'base64'));
    return out.counts;
}

const results = [];
try {
    const queue = Array.from({ length: runs }, (_, k) => views.map(view => [view, k])).flat();
    let next = 0;
    await Promise.all(Array.from({ length: pages }, async () => {
        while (next < queue.length) {
            const [view, k] = queue[next++];
            results.push(await run(view, k).catch(error => ({ name: view.name, run: k, error: error.message })));
        }
    }));
    writeFileSync(join(values.out, 'hashes.json'), JSON.stringify(results, null, 1));
    const lines = [`Runs per view: ${runs}, pages at once: ${pages}, query: ${values.query ?? '(none)'}`, '',
        '| View | Runs | Regions in every run | Differ (h/m/r/p/w) | In some runs | Unsettled | Pixels ≠ run 0 (max, >8 max) |', '| --- | --- | --- | --- | --- | --- | --- |'];
    const details = [];
    for (const { name } of views) {
        const done = results.filter(r => r.name === name && r.hashes).sort((a, b) => a.run - b.run);
        const failed = results.filter(r => r.name === name && r.error);
        if (!done.length) {
            lines.push(`| ${name} | 0 | | | | | ${failed.map(f => f.error).join('; ')} |`);
            continue;
        }
        const regions = new Set(done.flatMap(r => Object.keys(r.hashes)));
        const every = [...regions].filter(region => done.every(r => region in r.hashes));
        const differ = every.filter(region => new Set(done.map(r => r.hashes[region])).size > 1);
        const kinds = ['h', 'm', 'r', 'p', 'w'].map(kind => differ.filter(region => region.startsWith(`${kind}/`)).length).join('/');
        const unsettled = new Set(done.flatMap(r => r.unsettled));
        const pixels = done.length > 1 ? await pixelDiffs(done.map(r => r.file), join(values.out, name, 'diff.png')) : [];
        const max = pixels.reduce((m, [any, big]) => [Math.max(m[0], any), Math.max(m[1], big)], [0, 0]);
        lines.push(`| ${name} | ${done.length}${failed.length ? ` (+${failed.length} failed)` : ''} | ${every.length} | ${differ.length} (${kinds}) | ${regions.size - every.length} | ${unsettled.size} | ${max[0]}, ${max[1]} |`);
        for (const region of differ) {
            const groups = new Map();
            done.forEach(r => groups.set(r.hashes[region], [...(groups.get(r.hashes[region]) ?? []), r.run]));
            details.push(`- ${name} ${region}: ${[...groups.values()].map(g => `runs ${g.join(',')}`).join(' | ')}`);
        }
        if (unsettled.size) details.push(`- ${name} unsettled: ${[...unsettled].slice(0, 20).join(', ')}`);
        if (pixels.length) details.push(`- ${name} pixels vs run 0: ${pixels.map(([any, big]) => `${any}/${big}`).join(' ')}`);
    }
    const report = [...lines, '', ...details].join('\n');
    writeFileSync(join(values.out, 'report.md'), `${report}\n`);
    console.log(report);
} finally {
    await browser.close();
    close();
}
