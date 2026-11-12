// Renders the home views of a list with the TrailView still function, uploads them as one set, and
// makes a set the live set (specs/home-views.md). README.md beside this file has the steps.
//
//   node trailview/home-views.mjs render [--view ID] [--out DIR] [--preview] [--gpu] [--list FILE] [--fixture]
//   node trailview/home-views.mjs upload [--out DIR] [--list FILE]
//   node trailview/home-views.mjs switch SET
//
// --list is the list of views (default: home-views.json). render writes <out>/<date>/<id>/<slot>.webp
// and view.json, and prints the seconds of each image; --preview writes 640 × 360 images under
// <out>/preview instead. --gpu draws on the machine's GPU, else SwiftShader draws on the CPU, for
// minutes. --fixture answers the tile hosts from the fixture: build the page with `npm run
// pretest:trailview` first. upload and switch call `obc data r2`.

import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';
import { pathToFileURL } from 'node:url';
import { parseArgs } from 'node:util';
import { clock, daylight, instantAt, solarPosition, solarTerms } from '../../../src/lib/planner/solar.ts';
import { launch, routeFixture, serve } from './harness.js';

export const LIST = new URL('./home-views.json', import.meta.url).pathname;
const OUT = new URL('../../../../../.artifacts/home-views', import.meta.url).pathname;
/** The clock of TrailView. */
const TIME_ZONE = 'Europe/Zurich';
const SIZE = [2560, 1440], PREVIEW = [640, 360];
export const SLOTS = ['morning', 'noon', 'evening', 'night'];
/** Morning is this many minutes after sunrise, evening as many before sunset. */
const HOUR = 60;
/** The shortest day that gets three images, in minutes. */
const SHORTEST = 240;
/** The night image is at 23:00, and only where the sun is at least NIGHT_SUN degrees under the horizon then. */
const NIGHT = 23 * 60, NIGHT_SUN = -6;
/** After a tile fails, render waits this long and tries the image once more: the tile host answers 429 to too many requests. */
const RETRY_MS = 60_000;
/** The longest still on SwiftShader, which draws for minutes; on a GPU the default of `still()` holds. */
const SOFTWARE_MS = 600_000;
export const PREFIX = 'home-views', HOST = 'https://maps.openbikecomputer.com';
const IMMUTABLE = 'public, max-age=31536000, immutable', LIVE = 'public, max-age=60, must-revalidate';
const RAD = Math.PI / 180;

/** The list of a file: `{date, views: [{id, name, region, view, night?}]}`. Throws with each problem. */
export function readList(file = LIST) {
    const list = JSON.parse(readFileSync(file, 'utf8'));
    const problems = [];
    if (!/^\d{4}-\d{2}-\d{2}$/.test(list.date ?? '')) problems.push('date is not YYYY-MM-DD');
    if (!Array.isArray(list.views) || !list.views.length) problems.push('views is empty');
    const ids = new Set();
    for (const [i, view] of (list.views ?? []).entries()) {
        const at = `views[${i}]`;
        if (!/^[a-z0-9-]+$/.test(view.id ?? '')) problems.push(`${at}: id is not lowercase letters, digits and hyphens`);
        if (ids.has(view.id)) problems.push(`${at}: id ${view.id} is not unique`);
        ids.add(view.id);
        for (const key of ['name', 'region']) if (typeof view[key] !== 'string' || !view[key].trim()) problems.push(`${at}: ${key} is empty`);
        if (typeof view.view !== 'string' || !view.view.startsWith('?') || Object.values(place(view)).some(Number.isNaN)) problems.push(`${at}: view is not a query string with lat and lon`);
        if (view.night !== undefined && typeof view.night !== 'boolean') problems.push(`${at}: night is not true or false`);
    }
    if (problems.length) throw new Error(`${file}:\n${problems.join('\n')}`);
    return list;
}

const place = ({ view }) => {
    const query = new URLSearchParams(view);
    return { lat: Number(query.get('lat') ?? NaN), lon: Number(query.get('lon') ?? NaN) };
};

/** The slots of a view, in their order. */
export const slotsOf = view => SLOTS.filter(slot => slot !== 'night' || view.night);

/**
 * The sunrise and the sunset of a view on `date`, and the clock time of each slot, on the clock of
 * TrailView. Throws when the sun does not rise or does not set, when the day is shorter than SHORTEST,
 * or when the view has `night` and the sun is less than NIGHT_SUN under the horizon at 23:00.
 */
export function times(view, date) {
    const { lat, lon } = place(view);
    const altitude = minute => solarPosition([lon, lat], solarTerms(instantAt(date, minute, TIME_ZONE))).altitude / RAD;
    const span = daylight([lon, lat], date, TIME_ZONE);
    if (!span || span[0] === 0 || span[1] === 1440) throw new Error(`${view.id}: the sun does not rise or does not set on ${date}.`);
    const [rise, set] = span;
    if (set - rise < SHORTEST) throw new Error(`${view.id}: the day is shorter than ${SHORTEST / 60} hours on ${date}.`);
    if (view.night && altitude(NIGHT) > NIGHT_SUN) throw new Error(`${view.id}: the sun is less than ${-NIGHT_SUN}° under the horizon at ${clock(NIGHT)} on ${date}, so the view has no night.`);
    let noon = rise, highest = -Infinity;
    for (let minute = rise; minute <= set; minute++) {
        const a = altitude(minute);
        if (a > highest) [noon, highest] = [minute, a];
    }
    const slots = { morning: clock(rise + HOUR), noon: clock(noon), evening: clock(set - HOUR), night: clock(NIGHT) };
    return { sunrise: clock(rise), sunset: clock(set), slots: Object.fromEntries(slotsOf(view).map(slot => [slot, slots[slot]])) };
}

/** The manifest of a list with an empty `set`; `credits` holds the credits of each view by its id. */
export function manifestOf(list, credits) {
    return {
        format: 1,
        set: '',
        date: list.date,
        width: SIZE[0],
        height: SIZE[1],
        credits: [...new Set(list.views.flatMap(view => credits[view.id]))],
        views: list.views.map(view => {
            const { sunrise, sunset } = times(view, list.date);
            return { id: view.id, name: view.name, region: view.region, ...place(view), sunrise, sunset, night: Boolean(view.night) };
        }),
    };
}

/** The id of a set: the start of the SHA-256 of its manifest with an empty `set` and of its images, in the order of the views and the slots. */
export function setOf(manifest, images) {
    const hash = createHash('sha256').update(JSON.stringify({ ...manifest, set: '' }));
    for (const image of images) hash.update(image);
    return hash.digest('hex').slice(0, 16);
}

const readJson = file => (existsSync(file) ? JSON.parse(readFileSync(file, 'utf8')) : undefined);

/** Whether the folder of a view holds its images and a view.json of the same `view` and `night`. */
function rendered(dir, view) {
    const was = readJson(join(dir, 'view.json'));
    return was?.view === view.view && was.night === Boolean(view.night) && slotsOf(view).every(slot => existsSync(join(dir, `${slot}.webp`)));
}

/** One still in the page, as base64 WebP, or the name and message of its failure. */
const shoot = (page, job) => page.evaluate(async ({ search, date, time, width, height, timeoutMs }) => {
    const { view, still } = window.trailviewStill, started = performance.now();
    try {
        const { image, credits } = await still({ view: { ...view(search), date, time, debug: 'off' }, width, height }, { timeoutMs });
        if (image.type !== 'image/webp') return { failure: 'type', message: `the browser encoded ${image.type}, not WebP` };
        const url = await new Promise(resolve => {
            const reader = new FileReader();
            reader.onload = () => resolve(reader.result);
            reader.readAsDataURL(image);
        });
        return { image: url.slice(url.indexOf(',') + 1), credits, seconds: (performance.now() - started) / 1000 };
    } catch (error) {
        return { failure: error.name, message: error.message };
    }
}, job);

export async function render({ list, out = OUT, only, preview = false, gpu = false, fixture = false }) {
    const views = only ? list.views.filter(view => view.id === only) : list.views;
    if (!views.length) throw new Error(`The list has no view ${only}.`);
    const root = preview ? join(out, 'preview', list.date) : join(out, list.date), [width, height] = preview ? PREVIEW : SIZE;
    // Refuse before the browser opens.
    const jobs = views.map(view => ({ view, dir: join(root, view.id), slots: times(view, list.date).slots }));
    const todo = jobs.filter(({ view, dir }) => {
        if (!rendered(dir, view)) return true;
        console.log(`${view.id}: skipped, its images exist`);
        return false;
    });
    if (!todo.length) return;
    const { origin, close } = await serve();
    const browser = await launch(gpu);
    try {
        for (const { view, dir, slots } of todo) {
            rmSync(dir, { recursive: true, force: true });
            mkdirSync(dir, { recursive: true });
            const page = await browser.newPage();
            if (fixture) await routeFixture(page);
            await page.goto(`${origin}/trailview.html?still`);
            await page.waitForFunction(() => window.trailviewStill || document.querySelector('.notice'), null, { timeout: 60_000 });
            if (!await page.evaluate(software => window.trailviewStill?.canRenderStills({ software }), !gpu)) throw new Error('This browser has no GPU for stills: run render with --gpu on a machine with a GPU.');
            const credits = new Set();
            for (const [slot, time] of Object.entries(slots)) {
                const job = { search: view.view, date: list.date, time, width, height, timeoutMs: gpu ? undefined : SOFTWARE_MS };
                let result = await shoot(page, job);
                if (result.failure === 'tiles-failed') {
                    console.log(`${view.id}/${slot}: ${result.message} Waiting ${RETRY_MS / 1000} s to try once more.`);
                    await sleep(RETRY_MS);
                    result = await shoot(page, job);
                }
                if (result.failure) throw new Error(`${view.id}/${slot}: ${result.message} The next run starts again at this view.`);
                const image = Buffer.from(result.image, 'base64');
                writeFileSync(join(dir, `${slot}.webp`), image);
                result.credits.forEach(credit => credits.add(credit));
                console.log(`${view.id}/${slot} at ${time}: ${result.seconds.toFixed(1)} s, ${Math.round(image.length / 1000)} kB`);
            }
            // Written last: a view without it renders again.
            writeFileSync(join(dir, 'view.json'), `${JSON.stringify({ view: view.view, night: Boolean(view.night), credits: [...credits] }, null, 2)}\n`);
            await page.close();
        }
    } finally {
        await browser.close();
        close();
    }
}

/** `obc data r2 ARGS`; throws when it fails. */
function obcR2(args) {
    const { status } = spawnSync('obc', ['data', 'r2', ...args], { stdio: 'inherit' });
    if (status !== 0) throw new Error(`obc data r2 ${args.join(' ')} failed`);
}

/** Throws unless the public host answers 200 for each image of a manifest. */
async function checkImages(manifest, fetch) {
    const failed = [];
    for (const view of manifest.views) {
        for (const slot of slotsOf(view)) {
            const url = `${HOST}/${PREFIX}/${manifest.set}/${view.id}/${slot}.webp`, { status } = await fetch(url, { method: 'HEAD' });
            if (status !== 200) failed.push(`${url}: ${status}`);
        }
    }
    if (failed.length) throw new Error(`The public host does not serve each image of set ${manifest.set}:\n${failed.join('\n')}`);
}

/** Puts the images and the manifest of the rendered set under its id, checks them on the public host, and gives the id. */
export async function upload({ list, out = OUT, r2 = obcR2, fetch = globalThis.fetch }) {
    const root = join(out, list.date), missing = [], credits = {}, images = [];
    for (const view of list.views) {
        const dir = join(root, view.id);
        if (!rendered(dir, view)) {
            missing.push(view.id);
            continue;
        }
        credits[view.id] = readJson(join(dir, 'view.json')).credits;
        for (const slot of slotsOf(view)) images.push({ key: `${view.id}/${slot}.webp`, file: join(dir, `${slot}.webp`) });
    }
    if (missing.length) throw new Error(`These views have no complete images for their view in the list: ${missing.join(', ')}. Run render first.`);
    const manifest = manifestOf(list, credits);
    manifest.set = setOf(manifest, images.map(({ file }) => readFileSync(file)));
    const file = join(root, 'manifest.json');
    writeFileSync(file, `${JSON.stringify(manifest, null, 2)}\n`);
    for (const image of images) r2(['put', '--immutable', '--content-type', 'image/webp', '--cache-control', IMMUTABLE, image.file, `${PREFIX}/${manifest.set}/${image.key}`]);
    r2(['put', '--immutable', '--content-type', 'application/json', '--cache-control', IMMUTABLE, file, `${PREFIX}/${manifest.set}/manifest.json`]);
    await checkImages(manifest, fetch);
    return manifest.set;
}

/** Checks the images of an uploaded set on the public host, then puts its manifest at the live key. */
export async function switchTo(set, { r2 = obcR2, fetch = globalThis.fetch } = {}) {
    if (!/^[0-9a-f]{16}$/.test(set ?? '')) throw new Error(`${set}: a set id is 16 hexadecimal digits.`);
    const response = await fetch(`${HOST}/${PREFIX}/${set}/manifest.json`);
    if (response.status !== 200) throw new Error(`The public host has no manifest of set ${set}: ${response.status}.`);
    const text = await response.text(), manifest = JSON.parse(text);
    if (manifest.set !== set) throw new Error(`The manifest of set ${set} names set ${manifest.set}.`);
    await checkImages(manifest, fetch);
    const dir = mkdtempSync(join(tmpdir(), 'home-views-')), file = join(dir, 'manifest.json');
    try {
        writeFileSync(file, text);
        r2(['put', '--content-type', 'application/json', '--cache-control', LIVE, file, `${PREFIX}/manifest.json`]);
        return set;
    } finally {
        rmSync(dir, { recursive: true, force: true });
    }
}

async function main() {
    const { values, positionals: [command, set] } = parseArgs({
        allowPositionals: true,
        options: {
            view: { type: 'string' },
            out: { type: 'string' },
            list: { type: 'string' },
            preview: { type: 'boolean' },
            gpu: { type: 'boolean' },
            fixture: { type: 'boolean' },
        },
    });
    if (command === 'switch') return console.log(`Set ${await switchTo(set)} is live.`);
    const list = readList(values.list), out = values.out ?? OUT;
    if (command === 'render') return render({ list, out, only: values.view, preview: values.preview, gpu: values.gpu, fixture: values.fixture });
    if (command === 'upload') return console.log(await upload({ list, out }));
    throw new Error('Usage: home-views.mjs render|upload|switch SET; see the head of this file.');
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
    await main().catch(error => {
        console.error(error.message);
        process.exitCode = 1;
    });
}
