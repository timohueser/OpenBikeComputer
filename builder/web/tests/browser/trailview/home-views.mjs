// Renders the home views of a list with the TrailView still function, uploads them as one set, and
// makes a set the live set (specs/home-views.md). README.md beside this file has the steps.
//
//   node trailview/home-views.mjs render [--view ID] [--out DIR] [--preview] [--gpu] [--list FILE] [--fixture]
//   node trailview/home-views.mjs upload [--out DIR] [--list FILE]
//   node trailview/home-views.mjs switch SET
//
// --list is the list of views (default: home-views.json). render writes <out>/<date>/<id>/<slot>.webp
// and view.json, with the release of the page, and prints the seconds of each image; --preview writes
// 640 × 360 images under <out>/preview instead. --gpu draws on the machine's GPU, else SwiftShader
// draws on the CPU, for minutes. --fixture answers the tile hosts from the fixture: build the page
// with `npm run pretest:trailview` first. upload takes only images of the active production release.
// upload and switch call `obc data r2`.

import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
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
export const PREFIX = 'home-views', HOST = 'https://maps.openbikecomputer.com', CATALOG = `${HOST}/planner/catalog.json`;
const IMMUTABLE = 'public, max-age=31536000, immutable', LIVE = 'public, max-age=60, must-revalidate';
const RAD = Math.PI / 180;
/** The altitude of the sun's centre in degrees when its upper limb meets the horizon, as daylight() takes it. */
const SUNSET = -0.833;

/**
 * The list of a file: `{date, views: [{id, name, region, view, night?}]}`. `view` can be the whole
 * address that Ctrl+C copies: the list keeps the part from the `?`, as the paste of the page does.
 * Throws with each problem.
 */
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
        if (typeof view.view === 'string' && view.view.includes('?')) view.view = view.view.trim().slice(view.view.trim().indexOf('?'));
        if (typeof view.view !== 'string' || !view.view.startsWith('?') || Object.values(place(view)).some(Number.isNaN)) problems.push(`${at}: view is not an address or a query string with lat and lon`);
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
 * TrailView. Throws when the sun does not rise or does not set, when it is up at midnight on that
 * clock, when the day is shorter than SHORTEST, or when the view has `night` and the sun is less than
 * NIGHT_SUN under the horizon at 23:00.
 */
export function times(view, date) {
    const { lat, lon } = place(view);
    const altitude = minute => solarPosition([lon, lat], solarTerms(instantAt(date, minute, TIME_ZONE))).altitude / RAD;
    const span = daylight([lon, lat], date, TIME_ZONE);
    if (!span) throw new Error(`${view.id}: the sun does not rise on ${date}.`);
    if (span[0] === 0 || span[1] === 1440) {
        // daylight() gives one span from the first to the last minute of sun, also across a night.
        const sets = Array.from({ length: 24 }, (_, hour) => hour * 60 + 30).some(minute => {
            try {
                return altitude(minute) < SUNSET;
            } catch {
                // The clock skips this hour when it changes to summer time.
                return false;
            }
        });
        throw new Error(sets ? `${view.id}: the sun is up at midnight on the clock of ${TIME_ZONE} on ${date}.` : `${view.id}: the sun does not set on ${date}.`);
    }
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

/** Whether the folder of a view holds its images and a view.json of the same `view`, `night` and `release`. */
function rendered(dir, view, release) {
    const was = readJson(join(dir, 'view.json'));
    return was?.view === view.view && was.night === Boolean(view.night) && was.release === release && slotsOf(view).every(slot => existsSync(join(dir, `${slot}.webp`)));
}

/** A page with the still function; throws when the browser has no GPU for stills. */
async function stillPage(browser, origin, { gpu, fixture }) {
    const page = await browser.newPage();
    if (fixture) await routeFixture(page);
    await page.goto(`${origin}/trailview.html?still`);
    await page.waitForFunction(() => window.trailviewStill || document.querySelector('.notice'), null, { timeout: 60_000 });
    if (!await page.evaluate(software => window.trailviewStill?.canRenderStills({ software }), !gpu)) throw new Error('This browser has no GPU for stills: run render with --gpu on a machine with a GPU.');
    return page;
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
    const root = join(resolve(out), ...(preview ? ['preview'] : []), list.date), [width, height] = preview ? PREVIEW : SIZE;
    // Refuse before the browser opens.
    const jobs = views.map(view => ({ view, dir: join(root, view.id), slots: times(view, list.date).slots }));
    const { origin, close } = await serve();
    const browser = await launch(gpu);
    try {
        const probe = await stillPage(browser, origin, { gpu, fixture });
        const release = fixture ? 'fixture' : await probe.evaluate(() => window.trailviewStill.release);
        await probe.close();
        if (!release) throw new Error('The page has no release id: build it against a catalogue, not a config.');
        const todo = jobs.filter(({ view, dir }) => {
            if (!rendered(dir, view, release)) return true;
            console.log(`${view.id}: skipped, its images of release ${release} exist`);
            return false;
        });
        for (const { view, dir, slots } of todo) {
            rmSync(dir, { recursive: true, force: true });
            mkdirSync(dir, { recursive: true });
            const page = await stillPage(browser, origin, { gpu, fixture });
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
            writeFileSync(join(dir, 'view.json'), `${JSON.stringify({ view: view.view, night: Boolean(view.night), release, credits: [...credits] }, null, 2)}\n`);
            await page.close();
        }
    } finally {
        await browser.close();
        close();
    }
}

/** `obc data r2 ARGS`, which gives its standard output; throws when it fails. */
function obcR2(args) {
    const { status, stdout } = spawnSync('obc', ['data', 'r2', ...args], { stdio: ['inherit', 'pipe', 'inherit'], encoding: 'utf8' });
    if (status !== 0) throw new Error(`obc data r2 ${args.join(' ')} failed`);
    if (!args.includes('--json')) process.stdout.write(stdout);
    return stdout;
}

/** The keys of the images of a manifest. */
const imageKeys = manifest => manifest.views.flatMap(view => slotsOf(view).map(slot => `${PREFIX}/${manifest.set}/${view.id}/${slot}.webp`));

/** Throws unless the public host answers 200 for each image of a manifest. */
async function checkHost(manifest, fetch) {
    const failed = [];
    for (const key of imageKeys(manifest)) {
        const { status } = await fetch(`${HOST}/${key}`, { method: 'HEAD' });
        if (status !== 200) failed.push(`${HOST}/${key}: ${status}`);
    }
    if (failed.length) throw new Error(`The public host does not serve each image of set ${manifest.set}:\n${failed.join('\n')}`);
}

/** The id of the active release of the production catalogue. */
async function productionRelease(fetch) {
    const response = await fetch(CATALOG);
    if (response.status !== 200) throw new Error(`${CATALOG}: ${response.status}.`);
    return JSON.parse(await response.text()).active.id;
}

/** Puts the images and the manifest of the rendered set under its id, checks them on the public host, and gives the id. */
export async function upload({ list, out = OUT, r2 = obcR2, fetch = globalThis.fetch }) {
    // `obc data` runs from the root of the repository, not from this directory.
    const root = join(resolve(out), list.date), release = await productionRelease(fetch), missing = [], credits = {}, images = [];
    for (const view of list.views) {
        const dir = join(root, view.id);
        if (!rendered(dir, view, release)) {
            missing.push(view.id);
            continue;
        }
        credits[view.id] = readJson(join(dir, 'view.json')).credits;
        for (const slot of slotsOf(view)) images.push({ key: `${view.id}/${slot}.webp`, file: join(dir, `${slot}.webp`) });
    }
    if (missing.length) throw new Error(`These views have no complete images of their view in the list and of the production release ${release}: ${missing.join(', ')}. Run render first.`);
    const manifest = manifestOf(list, credits);
    manifest.set = setOf(manifest, images.map(({ file }) => readFileSync(file)));
    const file = join(root, 'manifest.json');
    writeFileSync(file, `${JSON.stringify(manifest, null, 2)}\n`);
    for (const image of images) r2(['put', '--immutable', '--content-type', 'image/webp', '--cache-control', IMMUTABLE, image.file, `${PREFIX}/${manifest.set}/${image.key}`]);
    r2(['put', '--immutable', '--content-type', 'application/json', '--cache-control', IMMUTABLE, file, `${PREFIX}/${manifest.set}/manifest.json`]);
    await checkHost(manifest, fetch);
    return manifest.set;
}

/**
 * Reads the manifest of an uploaded set from the bucket, checks that the bucket holds each of its
 * images, then puts the manifest at the live key. The bucket, not the public host: the edge cache
 * keeps an image for a year after it is deleted.
 */
export async function switchTo(set, { r2 = obcR2 } = {}) {
    if (!/^[0-9a-f]{16}$/.test(set ?? '')) throw new Error(`${set}: a set id is 16 hexadecimal digits.`);
    const key = `${PREFIX}/${set}/manifest.json`, held = keys => new Set(JSON.parse(r2(['stat', '--json', ...keys])).objects.map(object => object.key));
    if (!held([key]).size) throw new Error(`The bucket has no ${key}.`);
    const dir = mkdtempSync(join(tmpdir(), 'home-views-')), file = join(dir, 'manifest.json');
    try {
        r2(['get', key, file]);
        const manifest = JSON.parse(readFileSync(file, 'utf8'));
        if (manifest.set !== set) throw new Error(`The manifest of set ${set} names set ${manifest.set}.`);
        const images = imageKeys(manifest), there = held(images), missing = images.filter(image => !there.has(image));
        if (missing.length) throw new Error(`The bucket does not hold each image of set ${set}:\n${missing.join('\n')}`);
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
