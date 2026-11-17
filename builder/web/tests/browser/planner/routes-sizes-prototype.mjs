// Prototype for #3559: seeds a browser profile with real routed routes in Central Switzerland, rides, and real TrailView
// covers (SwiftShader, slow), against the dev server on :5317 and the production release.
//   node routes-sizes-prototype.mjs seed|covers|shots PROFILE_DIR OUT_DIR
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { chromium } from 'playwright';
import { SWIFTSHADER } from '../trailview/harness.js';

const [step, profile, out] = process.argv.slice(2);
const ORIGIN = 'http://localhost:5317';
const [W, H] = (process.env.COVER_SIZE ?? '960x640').split('x').map(Number);
const day = (date) => Date.parse(`${date}T08:00:00Z`);

// [id, name, bike, profile, days, updated, waypoints [lon, lat]]
const ROUTES = [
    ['klausen', 'Klausenpass from Altdorf', 'road', 'road', 1, '2026-07-04', [[8.644, 46.881], [8.855, 46.868]]],
    ['lucerne', 'Lake Lucerne loop', 'road', 'road', 1, '2026-06-20', [[8.309, 47.050], [8.440, 47.085], [8.610, 46.995], [8.540, 46.993], [8.420, 47.035], [8.309, 47.050]]],
    ['glaubenberg', 'Glaubenbielen gravel', 'gravel', 'gravel', 1, '2026-08-12', [[8.017, 46.952], [8.035, 46.822], [8.090, 46.818], [8.185, 46.835]]],
    ['grimsel', 'To the Grimsel', 'touring', 'touring', 3, '2026-07-28', [[8.309, 47.050], [8.245, 46.897], [8.140, 46.757], [8.187, 46.727], [8.333, 46.562]]],
    ['jochpass', 'Jochpass enduro', 'mtb', 'mtb', 1, '2026-08-30', [[8.405, 46.820], [8.390, 46.792], [8.420, 46.775], [8.360, 46.770]]],
    ['rigi', 'Rigi Kulm from Vitznau', 'hiking', 'hiking', 1, '2026-09-06', [[8.483, 47.010], [8.485, 47.056]]],
    ['pilatus', 'Pilatus via Alpnachstad', 'hiking', 'hiking/alpine', 1, '2026-09-13', [[8.277, 46.955], [8.254, 46.979]]],
    ['zug', 'Around Lake Zug', 'touring', 'touring', 1, '2026-05-30', [[8.515, 47.170], [8.520, 47.065], [8.430, 47.120], [8.460, 47.180], [8.515, 47.170]]],
    ['susten', 'Sustenpass', 'road', 'road', 1, '2026-08-02', [[8.600, 46.706], [8.447, 46.730], [8.230, 46.705]]],
    ['aegeri', 'Ägerital gravel loop', 'gravel', 'gravel', 1, '2026-10-03', [[8.585, 47.135], [8.650, 47.130], [8.630, 47.080], [8.585, 47.135]]],
    ['stoos', 'Stoos from Schwyz', 'mtb', 'mtb', 1, '2026-10-10', [[8.652, 47.021], [8.663, 46.977]]],
];
/** A route with no cover yet: the open question of a row with no picture. */
const NO_COVER = 'stoos';

async function seed(page) {
    await page.goto(`${ORIGIN}/planner.html?cover=none`);
    await page.waitForFunction(() => window.coverTest);
    const done = await page.evaluate(async ({ ROUTES, days, real }) => {
        const { library } = window.coverTest;
        const { requestRoute } = await import('/src/lib/planner/routing.ts');
        const { summarize } = await import('/src/lib/library/summary.ts');
        const lines = {};
        for (const [id, name, bike, profile, tripDays, updated, waypoints] of ROUTES) {
            const [route] = await requestRoute(waypoints, profile, new AbortController().signal);
            const numbers = { lengthM: Math.round(route.totals.distance_m), climbM: Math.round(route.totals.ascent_m), seconds: Math.round(route.totals.seconds), bike, days: tripDays };
            const summary = summarize(route.geometry.map(([lon, lat], i) => [lon, lat, route.elevation[i]]), numbers);
            const points = waypoints.map((coordinate, i) => ({ id: `${id}-${i}`, label: '', autoLabel: true, coordinate,
                kind: i === 0 ? 'start' : i === waypoints.length - 1 ? 'finish' : 'via', ...i > 0 && { leg: 'routed' } }));
            await library.saveRoute({ id, revision: (await library.route(id))?.revision ?? 0, name, updatedAt: days[id], trip: { mode: 'route', bike, days: tripDays, routeOrder: points.slice(1, -1).map(point => point.id), points }, summary });
            // A stored route keeps its stored name; only a rename changes it.
            if ((await library.route(id)).name !== name) await library.renameRoute(id, name);
            lines[id] = summary;
        }
        // The committed real cover of a route in Central Switzerland (src/lib/covers/testdata), a TrailView render at 480 × 320.
        const { summary: realSummary, points } = real;
        const realRoute = { id: 'real', revision: (await library.route('real'))?.revision ?? 0, name: 'Engelberg to Stansstad', updatedAt: Date.parse('2026-10-20T08:00:00Z'),
            trip: { mode: 'route', bike: 'touring', days: 1, routeOrder: [], points: [
                { id: 'r0', label: 'Engelberg', coordinate: realSummary.line.slice(0, 2), kind: 'start' },
                { id: 'r1', label: 'Stansstad', coordinate: realSummary.line.slice(-2), kind: 'finish', leg: 'routed' }] }, summary: realSummary };
        await library.saveRoute(realRoute);
        const { coverKey } = await import('/src/lib/covers/job.ts');
        await library.saveCover({ id: 'real', lineKey: coverKey(realRoute), state: 'done', tries: 0, image: new Blob([new Uint8Array(real.image)], { type: 'image/webp' }),
            credits: ['© swisstopo'], points: { x: Float32Array.from(points.x), y: Float32Array.from(points.y), eye: Uint8Array.from(points.eye) } });
        // A route that is not a plan: an import with a line and no trip that the planner can read.
        await library.saveRoute({ id: 'import', revision: (await library.route('import'))?.revision ?? 0, name: 'Imported GPX: Napf', updatedAt: days.zug - 1, trip: { nope: true }, summary: { ...lines.glaubenberg, bike: 'gravel' } });
        // Rides: a trip of three days on the Grimsel line, two rides of the Lake Lucerne loop (a group with its route), single rides.
        const ride = (key, name, summary, start, trip) => library.saveRide({ key, name, source: 'device', format: 'obcride', maxHr: null, ftp: null,
            summary: { ...summary, start, movingS: Math.round(summary.seconds * 0.85) }, ...trip && { trip } });
        const t = date => Date.parse(`${date}T07:00:00Z`) / 1000;
        const split = (summary, part, of) => {
            const n = summary.line.length / 2, a = Math.floor(n * part / of), b = Math.floor(n * (part + 1) / of) + 1;
            return { ...summary, line: summary.line.slice(2 * a, 2 * b), heights: summary.heights.slice(a, b), lengthM: Math.round(summary.lengthM / of), climbM: Math.round(summary.climbM / of), seconds: Math.round(summary.seconds / of), days: undefined };
        };
        for (let d = 0; d < 3; d++) await ride(`202607${28 + d}T070000Z`, `Grimsel day ${d + 1}`, split(lines.grimsel, d, 3), t(`2026-07-${28 + d}`), { key: '7001', day: d + 1, name: 'To the Grimsel' });
        await ride('20260621T070000Z', 'Lake Lucerne loop', lines.lucerne, t('2026-06-21'));
        await ride('20260809T070000Z', 'Lake Lucerne loop', lines.lucerne, t('2026-08-09'));
        await ride('20260705T070000Z', 'Klausen morning', lines.klausen, t('2026-07-05'));
        await ride('20260907T070000Z', 'Rigi hike', lines.rigi, t('2026-09-07'));
        await ride('20260831T070000Z', 'Jochpass', lines.jochpass, t('2026-08-31'));
        await library.setState('waiting', ['klausen', 'rigi']);
        await library.setState('sent', { lucerne: { device: 'card:7', key: 'x' } });
        return Object.keys(lines).length;
    }, { ROUTES, days: Object.fromEntries(ROUTES.map(r => [r[0], day(r[5])])), real: {
        ...JSON.parse(readFileSync(new URL('../../../src/lib/covers/testdata/cover.json', import.meta.url), 'utf8')),
        image: [...readFileSync(new URL('../../../src/lib/covers/testdata/cover.webp', import.meta.url))] } });
    console.log('seeded', done, 'routes');
}

/** Renders each missing cover in the TrailView still page, keeps it in OUT/covers, and stores it in the library. */
async function covers(context) {
    const dir = join(out, 'covers');
    mkdirSync(dir, { recursive: true });
    const planner = await context.newPage();
    await planner.goto(`${ORIGIN}/planner.html?cover=none`);
    await planner.waitForFunction(() => window.coverTest);
    const items = await planner.evaluate(async () => {
        await new Promise(r => setTimeout(r, 0));
        const { library } = window.coverTest;
        return [...await library.routes(), ...await library.rides()].map(item => ({ id: item.id ?? item.key, updatedAt: item.updatedAt, ride: 'key' in item, summary: item.summary }));
    });
    console.log('items', items.length);
    await planner.close();
    const stills = await context.newPage();
    stills.on('console', m => m.type() === 'error' && console.log('still:', m.text()));
    let requests = 0;
    stills.on('requestfailed', r => console.log('failed:', r.url().slice(0, 120), r.failure()?.errorText));
    stills.on('response', r => { requests++; if (r.status() >= 400) console.log('status', r.status(), r.url().slice(0, 120)); });
    const ticker = setInterval(() => console.log('requests', requests, new Date().toISOString()), 60_000);
    await stills.goto(`${ORIGIN}/trailview.html?still`);
    await stills.waitForFunction(() => window.trailviewStill, null, { timeout: 120_000 });
    console.log('software ok:', await stills.evaluate(() => window.trailviewStill.canRenderStills({ software: true })));
    for (const item of items.filter(item => !item.ride && item.summary && item.id !== NO_COVER && item.id !== 'import')) {
        const file = join(dir, `${item.id}.json`);
        if (!existsSync(file)) {
            const line = Array.from({ length: item.summary.line.length / 2 }, (_, i) => [item.summary.line[2 * i], item.summary.line[2 * i + 1]]);
            const date = new Date(item.updatedAt).toISOString().slice(0, 10);
            const started = Date.now();
            console.log(item.id, 'start', new Date().toISOString());
            const result = await stills.evaluate(async ({ line, date, W, H }) => {
                const { coverView, still } = window.trailviewStill;
                const view = await coverView(line, { width: W, height: H, date, time: '10:00', weather: 'sunny' });
                if (!view) return { none: true };
                const { image, points, credits } = await still({ view, width: W, height: H, line }, { timeoutMs: 1_200_000 });
                const url = await new Promise(resolve => { const r = new FileReader(); r.onload = () => resolve(r.result); r.readAsDataURL(image); });
                return { url, type: image.type, credits, points: { x: [...points.x], y: [...points.y], distance: [...points.distance], visible: [...points.visible] } };
            }, { line, date, W, H });
            console.log(item.id, result.none ? 'no view' : 'rendered', Math.round((Date.now() - started) / 1000), 's');
            writeFileSync(file, JSON.stringify(result));
        }
    }
    // Each route's cover; each ride takes the cover of the route whose line it has, or of the Grimsel route for its days.
    // The dev server reloads the page after an edit of the source, so the page loads again here.
    clearInterval(ticker);
    await stills.close();
    const store = await context.newPage();
    await store.goto(`${ORIGIN}/planner.html?cover=none`);
    await store.waitForFunction(() => window.coverTest);
    const saved = Object.fromEntries(items.filter(i => existsSync(join(dir, `${i.id}.json`))).map(i => [i.id, JSON.parse(readFileSync(join(dir, `${i.id}.json`), 'utf8'))]));
    await store.evaluate(async ({ saved }) => {
        const { library, lineKey } = window.coverTest;
        const { pack } = await import('/src/lib/covers/line.ts');
        const { coverKey } = await import('/src/lib/covers/job.ts');
        const routes = await library.routes(), rides = await library.rides();
        const source = item => {
            if (!('key' in item)) return saved[item.id];
            const match = routes.find(route => route.summary && route.summary.line.join() === item.summary.line.join());
            return saved[match?.id ?? 'grimsel'];
        };
        for (const item of [...routes, ...rides]) {
            const cover = source(item), id = item.id ?? item.key;
            if (!cover || cover.none) continue;
            const image = await (await fetch(cover.url)).blob();
            const p = cover.points;
            await library.saveCover({ id, lineKey: coverKey(item), state: 'done', tries: 0, image, credits: cover.credits,
                points: pack({ x: Float32Array.from(p.x), y: Float32Array.from(p.y), distance: Float32Array.from(p.distance), visible: Uint8Array.from(p.visible) }) });
        }
    }, { saved });
    console.log('covers stored');
}

/** One screenshot: the page at `query` and `hash`, with the row named `pick` selected; prints the rows in view. */
async function shot(context, { file, query = '', hash = '#/routes', width = 1440, height = 900, dark = false, pick, after }) {
    const page = await context.newPage();
    await page.setViewportSize({ width, height });
    await page.emulateMedia({ colorScheme: dark ? 'dark' : 'light' });
    await page.goto(`${ORIGIN}/planner.html${query}${hash}`);
    await page.locator('.list li button.row').first().waitFor();
    if (pick) await page.locator('.list button.row', { hasText: pick }).first().click();
    if (after) await after(page);
    await page.waitForTimeout(5000);
    const rows = await page.evaluate(() => {
        const list = document.querySelector('.list ul'), box = list.getBoundingClientRect();
        const visible = r => Math.max(0, Math.min(r.bottom, box.bottom, innerHeight) - Math.max(r.top, box.top, 0)) / r.height;
        const shares = [...list.querySelectorAll('li')].map(li => visible(li.getBoundingClientRect()));
        return { full: shares.filter(s => s > 0.99).length, partly: shares.reduce((a, s) => a + s, 0).toFixed(1), height: Math.round(list.querySelector('li')?.getBoundingClientRect().height ?? 0) };
    });
    console.log(file, JSON.stringify(rows));
    await page.screenshot({ path: join(out, file) });
    await page.close();
}

async function shots(context) {
    const pick = 'Engelberg';
    const all = [
        { file: 'today-1440.png', pick },
        { file: 'row1-1440.png', query: '?row=1', pick },
        { file: 'row2-1440.png', query: '?row=2', pick },
        ...[1, 2].flatMap(row => [180, 200, 240].map(card => ({ file: `row${row}-card${card}-1440.png`, query: `?row=${row}&card=${card}`, pick }))),
    ];
    const best = process.env.BEST ?? '?row=1&card=200', name = best.replace(/[?&]/g, '-').replace(/=/g, '').slice(1);
    all.push(
        { file: `${name}-dark-1440.png`, query: best, pick, dark: true },
        { file: `${name}-1100.png`, query: best, pick, width: 1100 },
        { file: `${name}-390.png`, query: best, pick, width: 390, height: 844 },
        { file: `${name}-ridden-1440.png`, query: best, hash: '#/routes/ridden', pick: 'To the Grimsel' },
        { file: `${name}-ridden-group-1440.png`, query: best, hash: '#/routes/ridden', pick: 'Lake Lucerne loop' },
        { file: `q-no-cover-map-picture-${name}-1440.png`, query: best, pick: 'Stoos' },
        { file: `q-no-cover-plain-line-${name}-1440.png`, query: `${best}&rowpic=none`, pick: 'Stoos' },
        { file: `q-not-a-plan-${name}-1440.png`, query: best, pick: 'Imported GPX' },
        { file: `q-card-hover-button-${name}-1440.png`, query: best, pick, after: page => page.getByRole('link', { name: 'Open in planner' }).hover() },
        { file: `q-card-hover-text-${name}-1440.png`, query: best, pick, after: async page => { const b = await page.locator('.selected .numbers').boundingBox(); await page.mouse.move(b.x + b.width / 2, b.y + b.height / 2); } },
    );
    const only = process.env.ONLY;
    for (const one of all) if (!only || one.file.includes(only)) await shot(context, one);
    // Does a click on the button open the planner, and a click on the text the Route page?
    const page = await context.newPage();
    await page.goto(`${ORIGIN}/planner.html${best}#/routes`);
    await page.locator('.list button.row', { hasText: pick }).first().click();
    await page.getByRole('link', { name: 'Open in planner' }).click();
    await page.waitForTimeout(1500);
    console.log('button click goes to', page.url());
    await page.goto(`${ORIGIN}/planner.html${best}#/routes`);
    await page.locator('.list button.row', { hasText: pick }).first().click();
    await page.locator('.selected .numbers').click({ force: true });
    await page.waitForTimeout(1500);
    console.log('text click goes to', page.url());
    await page.goto(`${ORIGIN}/planner.html${best}#/routes`);
    await page.locator('.list button.row', { hasText: 'Imported GPX' }).first().click();
    await page.getByRole('link', { name: 'Open in planner' }).click();
    await page.waitForTimeout(6000);
    console.log('not a plan: planner at', page.url());
    await page.screenshot({ path: join(out, `q-not-a-plan-opened-${name}-1440.png`) });
}

/** The map picture of each route and ride without a cover, in the light and the dark look: the cover job of the shell
 * makes them, as for a card, because it renders no cover on SwiftShader. */
async function maps(context) {
    for (const dark of [false, true]) {
        const page = await context.newPage();
        await page.emulateMedia({ colorScheme: dark ? 'dark' : 'light' });
        await page.goto(`${ORIGIN}/planner.html#/routes`);
        await page.locator('.list li button.row').first().waitFor();
        const ids = await page.evaluate(async () => {
            const { Library } = await import('/src/lib/library/store.ts');
            const library = new Library();
            return [...(await library.routes()).filter(r => r.summary).map(r => r.id), ...(await library.rides()).map(r => r.key)];
        });
        await page.evaluate(async ids => { const { wantCover } = await import('/src/lib/covers/job.ts'); for (const id of ids) wantCover(id, 'card'); }, ids);
        for (let i = 0; i < 120; i++) {
            const missing = await page.evaluate(async ({ ids, dark }) => {
                const { Library } = await import('/src/lib/library/store.ts');
                const library = new Library();
                const covers = await Promise.all(ids.map(id => library.cover(id)));
                return ids.filter((id, i) => !covers[i]?.image && !covers[i]?.map?.[dark ? 'dark' : 'light'] && !covers[i]?.map?.none);
            }, { ids, dark });
            console.log(dark ? 'dark' : 'light', 'missing', missing.length);
            if (!missing.length) break;
            await page.waitForTimeout(5000);
        }
        await page.close();
    }
}

const context = await chromium.launchPersistentContext(profile, { headless: true, args: SWIFTSHADER, viewport: { width: 1440, height: 900 }, deviceScaleFactor: 2 });
try {
    if (step === 'seed') await seed(await context.newPage());
    else if (step === 'covers') await covers(context);
    else if (step === 'shots') await shots(context);
    else if (step === 'maps') await maps(context);
} finally {
    await context.close();
}
