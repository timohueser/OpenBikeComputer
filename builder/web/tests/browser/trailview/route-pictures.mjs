// Renders the cover of each route of the route catalog with the TrailView still function, uploads
// the covers and the set that names them, and makes a set the live set (specs/route-pictures.md).
// README.md beside this file has the steps.
//
//   node trailview/route-pictures.mjs render [--gpu] [--fixture] [--out DIR] [--sample N] [--only ID,ID]
//   node trailview/route-pictures.mjs upload [--out DIR] [--without ID,ID]
//   node trailview/route-pictures.mjs switch SET
//
// render reads the catalog of the production release and writes <out>/covers/<id>/<key>-<light>.webp
// and <key>.json, or <key>.none for a route with no view. A route whose files exist is done. A route
// that fails gets no file: render goes on, names it at the end and exits with 1. At the end it writes
// the set of the covers in <out> under <out>/sets/<set>/. --sample N renders N routes spread over the
// catalog and prints what all of them would take. --gpu draws on the machine's GPU, else SwiftShader
// draws on the CPU, for minutes. --fixture reads fixture/routes.json and answers the tile hosts from
// the fixture: build the page with `npm run pretest:trailview` first. upload --without leaves these
// routes with no cover in the set. upload and switch call `obc data r2`.

import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';
import { pathToFileURL } from 'node:url';
import { parseArgs } from 'node:util';
import { launch, serve } from './harness.js';
import { HOST, IMMUTABLE, LIVE, RETRY_MS, SOFTWARE_MS, checkHost, obcR2, productionRelease, readJson, stillPage } from './stills.mjs';

const OUT = new URL('../../../../../.artifacts/route-pictures', import.meta.url).pathname;
export const FIXTURE_ROUTES = new URL('./fixture/routes.json', import.meta.url).pathname;
export const PREFIX = 'route-pictures';
const WIDTH = 960, HEIGHT = 640;
/** The version of the camera rule, and the light `MMDD-HHMM` with which `coverView` chooses the camera of a cover for each month of it: a change of one gives each line a new key. */
const CAMERA = 1, CAMERA_LIGHT = '0715-1000';
export const LOOK = `${CAMERA}|${CAMERA_LIGHT}|${WIDTH}|${HEIGHT}`;
/** The one month of the set, and the light of its images. */
const MONTH = '07', LIGHT = '0715-1000';
/** The code of `eye` (specs/route-pictures.md, `pack` in src/lib/covers/line.ts): 1 to 127 is NEAREST_M · 2^((c − 1) / STEPS) metres, plus VISIBLE. */
const NEAREST_M = 10, STEPS = 8, VISIBLE = 128;
/**
 * The time limit of one route in Node, where a hung page cannot stop it: the default limit of `still()`
 * (TIMEOUT_MS in src/lib/trailview/still.ts) or the job's, plus the 60 s that `stillPage` waits for the
 * page and 60 s for `coverView` and the encoding, which take seconds.
 */
const STILL_MS = 60_000, MARGIN_MS = 120_000;
/** After this many routes fail in a row, render stops unless a route with a cover renders again: the tile host or the GPU is down, not one area. */
const FAILURES_IN_A_ROW = 5;
/** Requests that run at a time, the wait after a 429 of the host and the most tries again. */
const AT_ONCE = 16, BUSY_MS = 60_000, BUSY_TRIES = 5;
/** The covers that upload checks on the public host. */
const CHECKED = 20;
/** The time limit of one points file from the public host. */
const POINTS_MS = 30_000;

/** The key of the cover of a record. */
export const keyOf = record => createHash('sha256').update(`${LOOK}|${record.line_udeg.join(',')}`).digest('hex').slice(0, 16);

/** The ends of the names of a cover's files after its key: the points, the mark of no view, the image of the month. */
const POINTS = '.json', NONE = '.none', IMAGE = `-${LIGHT}.webp`;
const coverDir = (out, record) => join(out, 'covers', String(record.id));
const coverFile = (out, record, end) => join(coverDir(out, record), `${keyOf(record)}${end}`);

/** 'cover' when the record's points and the image of the month are in `out`, 'none' when it has no view, else undefined. */
function stateOf(out, record) {
    if (existsSync(coverFile(out, record, POINTS)) && existsSync(coverFile(out, record, IMAGE))) return 'cover';
    if (existsSync(coverFile(out, record, NONE))) return 'none';
}

/** The sunny light of `MMDD-HHMM` in the year of the render; the day must be a day of each year. */
export function lightOf(light) {
    const [, month, day, hour, minute] = /^(\d\d)(\d\d)-(\d\d)(\d\d)$/.exec(light) ?? [];
    const date = month && new Date(Date.UTC(2027, month - 1, day));
    if (!date || date.getUTCMonth() !== month - 1 || date.getUTCDate() !== Number(day) || hour > 23 || minute > 59) throw new Error(`${light}: a light is MMDD-HHMM, on a day of each year.`);
    return { date: `${new Date().getFullYear()}-${month}-${day}`, time: `${hour}:${minute}`, weather: 'sunny' };
}

/**
 * The camera of a record's points file: from `out`, else from the public host when `fetch` is given, and
 * `out` keeps that file. Undefined when the key has no points yet. An image of a key renders only with this
 * camera, so that a new month matches the points that the bucket holds. Throws on any other answer than 200
 * with a whole camera or 404, and then writes nothing.
 */
export async function storedCamera(out, record, fetch) {
    const file = coverFile(out, record, POINTS);
    if (!existsSync(file) && fetch) {
        const url = `${HOST}/${PREFIX}/covers/${record.id}/${keyOf(record)}${POINTS}`;
        const text = await patiently(url, async () => {
            const response = await fetch(url, { signal: AbortSignal.timeout(POINTS_MS) });
            if (response.status === 404) return undefined;
            if (response.status !== 200) throw Object.assign(new Error(`${url} answers ${response.status}.`), { status: response.status });
            return response.text();
        });
        if (text === undefined) return undefined;
        const camera = JSON.parse(text).camera;
        if (!['lat', 'lon', 'heading', 'pitch', 'height'].every(field => Number.isFinite(camera?.[field]))) throw new Error(`${url} has no whole camera.`);
        mkdirSync(coverDir(out, record), { recursive: true });
        writeWhole(file, text);
    }
    return readJson(file)?.camera;
}

/** Writes a file under a temporary name first, so that a killed run leaves no half file. */
function writeWhole(file, data) {
    writeFileSync(`${file}.part`, data);
    renameSync(`${file}.part`, file);
}

/** The route records of catalog files, in `id` order; a long route has no line and no cover. */
const routesOf = files => [...new Map(files.flatMap(file => file.routes).filter(record => record.line_udeg).map(record => [record.id, record])).values()].sort((a, b) => a.id - b.id);

/**
 * The ids `9-X-Y` of the zoom 9 cells whose box overlaps `bounds` with a positive area, by the rule of
 * `coversCell` (src/lib/planner/map-data.ts): a margin keeps out a cell that only touches an edge.
 */
export function gridCells([west, south, east, north]) {
    const n = 512, margin = 1e-9, latitude = row => Math.atan(Math.sinh(Math.PI * (1 - 2 * row / n))) * 180 / Math.PI;
    const column = lon => Math.min(n - 1, Math.max(0, Math.floor((lon + 180) / 360 * n)));
    const row = lat => Math.min(n - 1, Math.max(0, Math.floor((1 - Math.asinh(Math.tan(lat * Math.PI / 180)) / Math.PI) / 2 * n)));
    const cells = [];
    for (let y = row(north); y <= row(south); y++) {
        for (let x = column(west); x <= column(east); x++) {
            const wide = Math.min(east, (x + 1) / n * 360 - 180) - Math.max(west, x / n * 360 - 180) > margin;
            if (wide && Math.min(north, latitude(y)) - Math.max(south, latitude(y + 1)) > margin) cells.push(`9-${x}-${y}`);
        }
    }
    return cells;
}

const busy = error => error.status === 429 || /\b429\b/.test(error.stderr ?? '');

/** Runs `attempt`; after a 429 of the host it waits BUSY_MS and tries again, BUSY_TRIES times at most. */
export async function patiently(name, attempt, wait = sleep) {
    for (let tries = 0; ; tries++) {
        try {
            return await attempt();
        } catch (error) {
            if (tries >= BUSY_TRIES || !busy(error)) throw error;
            console.log(`${name}: the host answers 429. Waiting ${BUSY_MS / 1000} s to try again.`);
            await wait(BUSY_MS);
        }
    }
}

/** Runs `work` on each item, `count` at a time; rejects with the first failure. */
async function pool(items, count, work) {
    let next = 0;
    await Promise.all(Array.from({ length: Math.min(count, items.length) }, async () => {
        while (next < items.length) await work(items[next++]);
    }));
}

/** The route records of a release: its one region file when `routes` names no cell, else the file of each grid cell. */
export async function releaseRoutes(release, fetch, wait) {
    const urls = release.routes.includes('{cell}') ? gridCells(release.bounds).map(cell => release.routes.replace('{cell}', cell)) : [release.routes];
    const files = [];
    await pool(urls, AT_ONCE, url => patiently(url, async () => {
        const response = await fetch(url);
        // A covered cell without a file is an error (specs/route-catalog.md, "Files").
        if (response.status !== 200) throw Object.assign(new Error(`${url}: ${response.status}.`), { status: response.status });
        files.push(JSON.parse(await response.text()));
    }, wait));
    return routesOf(files);
}

/** The `count` records of `records` that --sample takes, spread over them. */
export const sampleOf = (records, count) => (count >= records.length ? records : Array.from({ length: count }, (_, i) => records[Math.floor(i * records.length / count)]));

/** The line of a record as [lon, lat] degrees. */
function lineOf({ line_udeg: udeg }) {
    const line = [];
    for (let i = 0, lon = 0, lat = 0; i < udeg.length; i += 2) line.push([(lon += udeg[i]) / 1e6, (lat += udeg[i + 1]) / 1e6]);
    return line;
}

/** The Z-order of the centre of a line's box on a grid of 2^16 × 2^16 Web Mercator cells: near lines have near keys. */
export function placeKey(line) {
    const lons = line.map(p => p[0]), lats = line.map(p => p[1]), n = 2 ** 16;
    const lon = (Math.min(...lons) + Math.max(...lons)) / 2, lat = (Math.min(...lats) + Math.max(...lats)) / 2;
    const x = Math.floor((lon + 180) / 360 * n), y = Math.floor((1 - Math.asinh(Math.tan(lat * Math.PI / 180)) / Math.PI) / 2 * n);
    let key = 0;
    for (let bit = 15; bit >= 0; bit--) key = key * 4 + ((y >> bit) & 1) * 2 + ((x >> bit) & 1);
    return key;
}

/** The points file of a cover from the points of a still: one decimal of a pixel, and `null` and eye 0 for a point with no place. */
export function pointsOf({ x, y, distance, visible }, credits, { lat, lon, heading, pitch, height }) {
    const placed = i => [x[i], y[i], distance[i]].every(Number.isFinite), place = values => Array.from(values, (v, i) => (placed(i) ? Math.round(v * 10) / 10 : null));
    const eye = Array.from(distance, (d, i) => (placed(i) ? Math.min(127, 1 + Math.round(STEPS * Math.log2(Math.max(d, NEAREST_M) / NEAREST_M))) | (visible[i] ? VISIBLE : 0) : 0));
    return { x: place(x), y: place(y), eye, credits, camera: { lat, lon, heading, pitch, height } };
}

/** Throws unless the still page was built for the release whose routes render reads. */
export function checkRelease(page, release) {
    if (page !== release) throw new Error(`The still page has release ${page ?? 'none'}, not the production release ${release}: build it for production with \`npm run build:trailview --prefix ../..\`.`);
}

/**
 * The cover of one line in the page: the stored `camera`, else the camera of `coverView` in the camera light,
 * then the still in the light of the month; `none` when no view holds the line.
 */
const shoot = (page, job) => page.evaluate(async ({ line, camera, cameraLight, light, width, height, timeoutMs }) => {
    const { coverView, still } = window.trailviewStill;
    try {
        let view;
        try {
            view = camera ? { ...camera, mode: 'fly', debug: 'off', plan: 'off' } : await coverView(line, { width, height, ...cameraLight });
        } catch (error) {
            // `coverView` fails when it cannot read a terrain tile, for example after a 429: render waits and tries again as for a still.
            return { failure: 'tiles-failed', message: `the camera failed: ${error.message}` };
        }
        if (!view) return { none: true };
        const { image, points, credits } = await still({ view: { ...view, ...light }, width, height, line }, { timeoutMs });
        if (image.type !== 'image/webp') return { failure: 'type', message: `the browser encoded ${image.type}, not WebP` };
        const url = await new Promise(resolve => {
            const reader = new FileReader();
            reader.onload = () => resolve(reader.result);
            reader.readAsDataURL(image);
        });
        const arrays = Object.fromEntries(Object.entries(points).map(([key, values]) => [key, Array.from(values)]));
        return { image: url.slice(url.indexOf(',') + 1), points: arrays, credits, view };
    } catch (error) {
        return { failure: error.name, message: error.message };
    }
}, job);

/**
 * The cover of one line in a page of its own, with the seconds of the page and the still: the renderer keeps
 * the memory of each still until its page closes. After the time limit the route fails and its browser goes,
 * because a hung page may not close; `browser.get()` gives the context of the browser, and starts a new
 * browser, also after one died.
 */
async function cover(browser, origin, options, job) {
    const started = performance.now(), limit = (job.timeoutMs ?? STILL_MS) + MARGIN_MS;
    let page, timer;
    const late = new Promise(resolve => {
        timer = setTimeout(() => resolve({ failure: 'late', message: `the route took more than ${limit / 1000} s` }), limit);
    });
    const work = (async () => {
        page = await stillPage(await browser.get(), origin, options);
        return { ...await shoot(page, job), seconds: (performance.now() - started) / 1000 };
    // Playwright appends the whole log of the browser to its message.
    })().catch(error => ({ failure: 'page', message: `the page failed: ${error.message.split('\n')[0]}` }));
    const result = await Promise.race([work, late]);
    clearTimeout(timer);
    if (result.failure === 'late') await browser.drop();
    else await page?.close().catch(() => {});
    return result;
}

/** The id of a set: the hash of each cell id, a line feed and its cell file in the order of the cell ids, then of its manifest with an empty `set`. */
const setOf = (cells, manifest) => {
    const hash = createHash('sha256');
    for (const [id, file] of cells) hash.update(`${id}\n${file}`);
    return hash.update(JSON.stringify({ ...manifest, set: '' })).digest('hex').slice(0, 16);
};

/**
 * Writes the cell files and the manifest of the set of the covers in `out` for `records`, under
 * <out>/sets/<set>/, and gives the set id, the manifest and the cell ids.
 */
export function writeSet(out, records, release) {
    const cells = {}, credits = new Set();
    let covers = 0;
    for (const record of records) {
        if (stateOf(out, record) !== 'cover') continue;
        covers++;
        readJson(coverFile(out, record, POINTS)).credits.forEach(credit => credits.add(credit));
        for (const cell of record.cells) (cells[cell] ??= {})[record.id] = keyOf(record);
    }
    const ids = Object.keys(cells).sort(), files = ids.map(cell => `${JSON.stringify({ format: 1, routes: cells[cell] })}\n`);
    const manifest = { format: 1, set: '', release, look: LOOK, months: { [MONTH]: LIGHT }, default_month: MONTH, width: WIDTH, height: HEIGHT, covers, credits: [...credits] };
    manifest.set = setOf(ids.map((cell, i) => [cell, files[i]]), manifest);
    const dir = join(out, 'sets', manifest.set);
    mkdirSync(join(dir, 'cells'), { recursive: true });
    ids.forEach((cell, i) => writeFileSync(join(dir, 'cells', `${cell}.json`), files[i]));
    writeFileSync(join(dir, 'manifest.json'), `${JSON.stringify(manifest, null, 2)}\n`);
    return { set: manifest.set, manifest, cells: ids };
}

/** Renders the covers that `out` lacks and writes the set; gives the ids of the routes that failed. */
export async function render({ out = OUT, sample, only, gpu = false, fixture = false, fetch = globalThis.fetch }) {
    out = resolve(out);
    const active = fixture ? undefined : await productionRelease(fetch);
    const records = fixture ? routesOf([JSON.parse(readFileSync(FIXTURE_ROUTES, 'utf8'))]) : await releaseRoutes(active, fetch);
    let chosen = only ? records.filter(record => only.includes(record.id)) : records;
    if (sample) chosen = sampleOf(chosen, sample);
    // Neighbours render one after the other: their pages share the tiles in the HTTP cache and the bakes in the device cache of the context.
    const todo = chosen.filter(record => !stateOf(out, record)).map(record => [placeKey(lineOf(record)), record]).sort((a, b) => a[0] - b[0]).map(([, record]) => record);
    console.log(`${records.length} routes, ${chosen.length} chosen, ${todo.length} to render.`);
    const cameraLight = lightOf(CAMERA_LIGHT), light = lightOf(LIGHT);
    const { origin, close } = await serve();
    let current;
    const browser = {
        get: async () => (current?.browser().isConnected() ? current : (current = await (await launch(gpu)).newContext())),
        // Playwright kills a browser whose close takes more than 30 s.
        drop: async () => {
            const old = current;
            current = undefined;
            await old?.browser().close().catch(() => {});
        },
    };
    const options = { gpu, fixture }, release = active?.id ?? 'fixture', failed = [];
    let covers = 0, none = 0, seconds = 0, bytes = 0;
    try {
        const probe = await stillPage(await browser.get(), origin, options);
        if (!fixture) checkRelease(await probe.evaluate(() => window.trailviewStill.release), release);
        await probe.close();
        const jobOf = async record => ({ line: lineOf(record), camera: await storedCamera(out, record, !fixture && fetch), cameraLight, light, width: WIDTH, height: HEIGHT, timeoutMs: gpu ? undefined : SOFTWARE_MS });
        // A route with a cover from before this run checks the hosts: its tiles are not in the caches of this run.
        const before = records.find(record => stateOf(out, record) === 'cover');
        let inARow = 0, latest;
        for (const record of todo) {
            let job;
            try {
                job = await jobOf(record);
            } catch (error) {
                // Each next route would meet the same host, and the check after failures in a row makes no fetch.
                failed.push(`${record.id}: the points file on the public host failed: ${error.message}`);
                console.log(`${failed.at(-1)} render stops. Check the host, then run again.`);
                break;
            }
            let result = await cover(browser, origin, options, job);
            if (result.failure === 'tiles-failed') {
                console.log(`${record.id}: ${result.message} Waiting ${RETRY_MS / 1000} s to try once more.`);
                await sleep(RETRY_MS);
                result = await cover(browser, origin, options, job);
            }
            if (result.failure) {
                failed.push(`${record.id}: ${result.message}`);
                console.log(`${record.id}: failed: ${result.message}`);
                if (++inARow >= FAILURES_IN_A_ROW) {
                    // The routes of one area follow each other: their faults stop render only when a route with a cover fails too,
                    // in a new browser with empty caches.
                    const known = before ?? latest;
                    await browser.drop();
                    const check = known && await cover(browser, origin, options, await jobOf(known));
                    if (!check || check.failure) {
                        console.log(`${inARow} routes failed in a row${known ? `, and route ${known.id} with a cover too` : ''}: render stops. Check the tile host and the GPU, then run again.`);
                        break;
                    }
                    console.log(`${inARow} routes failed in a row, but route ${known.id} with a cover renders: render goes on.`);
                    inARow = 0;
                }
                continue;
            }
            inARow = 0;
            if (!result.none) latest = record;
            mkdirSync(coverDir(out, record), { recursive: true });
            seconds += result.seconds;
            if (result.none) {
                none++;
                writeFileSync(coverFile(out, record, NONE), '');
                console.log(`${record.id}: no view, ${result.seconds.toFixed(1)} s`);
                continue;
            }
            const image = Buffer.from(result.image, 'base64');
            writeWhole(coverFile(out, record, IMAGE), image);
            // A points file is never written again: the images of its other months come from its camera.
            if (!job.camera) writeWhole(coverFile(out, record, POINTS), JSON.stringify(pointsOf(result.points, result.credits, result.view)));
            covers++;
            bytes += image.length;
            console.log(`${record.id}: ${result.seconds.toFixed(1)} s, ${Math.round(image.length / 1000)} kB`);
        }
    } finally {
        await browser.drop();
        close();
    }
    const done = covers + none, mean = done ? seconds / done : 0;
    console.log(`${covers} covers, ${none} routes with no view, ${failed.length} failed, ${mean.toFixed(1)} s each, ${(bytes / 1e6).toFixed(1)} MB.`);
    if (sample && done) {
        const all = Math.round(records.length * covers / done), cells = new Set(records.flatMap(record => record.cells)).size;
        console.log(`All ${records.length} routes: about ${(records.length * mean / 3600).toFixed(1)} h, ${(all * bytes / Math.max(covers, 1) / 1e6).toFixed(0)} MB, ${all} covers; an upload of ${2 * all + cells + 1} files at most.`);
    }
    const { set } = writeSet(out, records, release);
    console.log(`Set ${set} in ${join(out, 'sets', set)}.`);
    if (failed.length) console.log(`These routes failed and have no file; the next run tries them again:\n${failed.join('\n')}`);
    return failed;
}

/** `r2` put with the headers of an immutable key, patiently. */
const put = (r2, type, file, key, wait) => patiently(key, () => r2(['put', '--immutable', '--content-type', type, '--cache-control', IMMUTABLE, file, key]), wait);

/** The keys in the bucket under the folder `folder`, from one list. `obc data r2` takes a folder with no final `/`. */
async function listed(r2, folder) {
    const keys = JSON.parse(await r2(['list', '--json', folder])).objects.map(object => object.key);
    return new Set(keys.filter(key => key.startsWith(`${folder}/`)));
}

/**
 * Makes the set of the covers in `out` for the production release, puts the covers that the bucket
 * does not hold, then the cell files and last the manifest of the set, checks them on the public
 * host, and gives the set id. The routes of `without` have no cover in the set.
 */
export async function upload({ out = OUT, without = [], r2 = obcR2, fetch = globalThis.fetch, wait = sleep }) {
    // `obc data` runs from the root of the repository, not from this directory.
    out = resolve(out);
    const release = await productionRelease(fetch), records = (await releaseRoutes(release, fetch, wait)).filter(record => !without.includes(record.id));
    const missing = records.filter(record => !stateOf(out, record)).map(record => record.id);
    if (missing.length) throw new Error(`These routes of the production release ${release.id} have no cover and no mark of no view: ${missing.join(', ')}. Run render first, or leave them out with --without.`);
    const { set, cells } = writeSet(out, records, release.id), dir = join(out, 'sets', set);
    const covered = records.filter(record => stateOf(out, record) === 'cover'), held = await listed(r2, `${PREFIX}/covers`);
    const filesOf = record => [['image/webp', IMAGE], ['application/json', POINTS]].map(([type, end]) => ({ type, file: coverFile(out, record, end), key: `${PREFIX}/covers/${record.id}/${keyOf(record)}${end}` }));
    const files = covered.flatMap(filesOf), sent = files.filter(({ key }) => !held.has(key));
    console.log(`${sent.length} of ${files.length} cover files to send.`);
    // The points first: an image in the bucket always has its points, so a later render takes its camera.
    for (const kind of ['application/json', 'image/webp']) await pool(sent.filter(({ type }) => type === kind), AT_ONCE, ({ type, file, key }) => put(r2, type, file, key, wait));
    await pool(cells, AT_ONCE, cell => put(r2, 'application/json', join(dir, 'cells', `${cell}.json`), `${PREFIX}/sets/${set}/cells/${cell}.json`, wait));
    // Last: a set whose manifest is in the bucket has each of its cell files there.
    await put(r2, 'application/json', join(dir, 'manifest.json'), `${PREFIX}/sets/${set}/manifest.json`, wait);
    const step = Math.max(1, Math.floor(covered.length / CHECKED)), checked = covered.filter((_, i) => i % step === 0).slice(0, CHECKED).flatMap(filesOf);
    const setKeys = [...cells.map(cell => `${PREFIX}/sets/${set}/cells/${cell}.json`), `${PREFIX}/sets/${set}/manifest.json`];
    await checkHost([...setKeys, ...checked.map(({ key }) => key)], fetch, `each file of set ${set}`);
    return set;
}

/** Throws unless each light of `months` is in its month and `default_month` is a month of the set. */
export function checkMonths({ months, default_month: month }) {
    for (const [key, light] of Object.entries(months)) {
        lightOf(light);
        if (light.slice(0, 2) !== key) throw new Error(`The light ${light} is not in its month ${key}.`);
    }
    if (!Object.hasOwn(months, month)) throw new Error(`The default_month ${month} is not a month of the set.`);
}

/**
 * Reads the cell files of an uploaded set from the bucket, checks that they and the manifest give the set id,
 * so that no cell file is missing, and with one list of the bucket that it holds the points and the image of
 * each month of the set of each cover that they name, then puts the manifest of the set at the live key. The
 * bucket, not the public host: the edge cache keeps a file for a year after it is deleted.
 */
export async function switchTo(set, { r2 = obcR2 } = {}) {
    if (!/^[0-9a-f]{16}$/.test(set ?? '')) throw new Error(`${set}: a set id is 16 hexadecimal digits.`);
    const held = await listed(r2, PREFIX), root = `${PREFIX}/sets/${set}/`, key = `${root}manifest.json`;
    if (!held.has(key)) throw new Error(`The bucket has no ${key}.`);
    const dir = mkdtempSync(join(tmpdir(), 'route-pictures-')), file = join(dir, 'manifest.json');
    try {
        await r2(['get', key, file]);
        const manifest = JSON.parse(readFileSync(file, 'utf8')), ends = [POINTS, ...Object.values(manifest.months).map(light => `-${light}.webp`)];
        if (manifest.set !== set) throw new Error(`The manifest of set ${set} names another set.`);
        checkMonths(manifest);
        const idOf = name => name.slice(`${root}cells/`.length, -'.json'.length);
        const cells = [...held].filter(name => name.startsWith(`${root}cells/`)).sort((a, b) => (idOf(a) < idOf(b) ? -1 : 1)), missing = new Set(), texts = new Map();
        await pool(cells, AT_ONCE, async cell => {
            const copy = join(dir, cell.slice(cell.lastIndexOf('/') + 1));
            await r2(['get', cell, copy]);
            texts.set(cell, readFileSync(copy, 'utf8'));
            for (const [id, cover] of Object.entries(JSON.parse(texts.get(cell)).routes)) {
                for (const name of ends.map(end => `${PREFIX}/covers/${id}/${cover}${end}`)) if (!held.has(name)) missing.add(name);
            }
        });
        if (setOf(cells.map(cell => [idOf(cell), texts.get(cell)]), manifest) !== set) throw new Error(`The cell files of set ${set} in the bucket do not give its id: one is missing or changed.`);
        if (missing.size) throw new Error(`The bucket does not hold each cover of set ${set}:\n${[...missing].sort().join('\n')}`);
        await r2(['put', '--content-type', 'application/json', '--cache-control', LIVE, file, `${PREFIX}/manifest.json`]);
        return set;
    } finally {
        rmSync(dir, { recursive: true, force: true });
    }
}

async function main() {
    const { values, positionals: [command, set] } = parseArgs({
        allowPositionals: true,
        options: {
            out: { type: 'string' },
            sample: { type: 'string' },
            only: { type: 'string' },
            without: { type: 'string' },
            gpu: { type: 'boolean' },
            fixture: { type: 'boolean' },
        },
    });
    const out = values.out ?? OUT, ids = text => text?.split(',').map(Number);
    if (command === 'switch') return console.log(`Set ${await switchTo(set)} is live.`);
    if (command === 'upload') return console.log(await upload({ out, without: ids(values.without) }));
    if (command === 'render') {
        const sample = values.sample === undefined ? undefined : Number(values.sample);
        if (sample !== undefined && !(Number.isInteger(sample) && sample > 0)) throw new Error('--sample takes a count of routes.');
        const failed = await render({ out, sample, only: ids(values.only), gpu: values.gpu, fixture: values.fixture });
        if (failed.length) process.exitCode = 1;
        return;
    }
    throw new Error('Usage: route-pictures.mjs render|upload|switch SET; see the head of this file.');
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
    await main().catch(error => {
        console.error(error.message);
        process.exitCode = 1;
    });
}
