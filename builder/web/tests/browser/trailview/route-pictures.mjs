// Renders the cover of each route of the route catalog with the TrailView still function, uploads
// the covers and the set that names them, and makes a set the live set (specs/route-pictures.md).
// README.md beside this file has the steps.
//
//   node trailview/route-pictures.mjs render [--gpu] [--fixture] [--catalog FILE] [--out DIR] [--sample N] [--only ID,ID]
//   node trailview/route-pictures.mjs upload [--out DIR]
//   node trailview/route-pictures.mjs switch SET
//
// render reads the catalog of the production release, or the region file --catalog, and writes
// <out>/covers/<id>/<key>.webp and .json, or <key>.none for a route with no view. A route whose
// files exist is done. At the end it writes the set of the covers in <out> under <out>/sets/<set>/.
// --sample N renders N routes spread over the catalog and prints what all of them would take.
// --gpu draws on the machine's GPU, else SwiftShader draws on the CPU, for minutes. --fixture
// answers the tile hosts from the fixture: build the page with `npm run pretest:trailview` first.
// upload and switch call `obc data r2`.

import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';
import { pathToFileURL } from 'node:url';
import { parseArgs } from 'node:util';
import { launch, serve } from './harness.js';
import { IMMUTABLE, LIVE, RETRY_MS, SOFTWARE_MS, checkHost, obcR2, productionRelease, readJson, stillPage } from './stills.mjs';

const OUT = new URL('../../../../../.artifacts/route-pictures', import.meta.url).pathname;
export const PREFIX = 'route-pictures';
const WIDTH = 960, HEIGHT = 640;
/** The version of the camera rule, and the date and the time of the light: a change of one gives each line a new key. */
const CAMERA = 1, DAY = '07-15', TIME = '10:00';
export const LOOK = `${CAMERA}|${DAY}|${TIME}|${WIDTH}|${HEIGHT}`;
/** The code of `eye` in `pack` (src/lib/covers/line.ts): 1 to 127 is NEAREST_M · 2^((c − 1) / STEPS) metres, plus VISIBLE. */
const NEAREST_M = 10, STEPS = 8, VISIBLE = 128;
/** Puts that run at a time, the wait after a 429 of the host and the most tries again. */
const PUTS = 16, BUSY_MS = 60_000, BUSY_TRIES = 5;
/** The covers that upload checks on the public host. */
const CHECKED = 20;

/** The key of the cover of a record. */
export const keyOf = record => createHash('sha256').update(`${LOOK}|${record.line_udeg.join(',')}`).digest('hex').slice(0, 16);

const coverDir = (out, record) => join(out, 'covers', String(record.id));
const coverFile = (out, record, ext) => join(coverDir(out, record), `${keyOf(record)}.${ext}`);

/** 'cover' when the record's cover is in `out`, 'none' when it has no view, else undefined. */
function stateOf(out, record) {
    if (['webp', 'json'].every(ext => existsSync(coverFile(out, record, ext)))) return 'cover';
    if (existsSync(coverFile(out, record, 'none'))) return 'none';
}

/** The route records of a catalog file, in `id` order; a long route has no line and no cover. */
const routesOf = files => [...new Map(files.flatMap(file => file.routes).filter(record => record.line_udeg).map(record => [record.id, record])).values()].sort((a, b) => a.id - b.id);

/** The ids `9-X-Y` of the zoom 9 cells whose box overlaps `bounds` with a positive area. */
export function gridCells([west, south, east, north]) {
    const n = 2 ** 9, x = lon => (lon + 180) / 360 * n;
    const y = lat => (1 - Math.asinh(Math.tan(lat * Math.PI / 180)) / Math.PI) / 2 * n;
    const cells = [];
    for (let j = Math.floor(y(north)); j < Math.ceil(y(south)); j++) for (let i = Math.floor(x(west)); i < Math.ceil(x(east)); i++) cells.push(`9-${i}-${j}`);
    return cells;
}

/** The route records of a grid release: the file of each grid cell. */
export async function releaseRoutes(release, fetch) {
    const files = await Promise.all(gridCells(release.bounds).map(async cell => {
        const url = release.routes.replace('{cell}', cell), response = await fetch(url);
        if (response.status !== 200) throw new Error(`${url}: ${response.status}. Each grid cell of release ${release.id} has a catalog file.`);
        return JSON.parse(await response.text());
    }));
    return routesOf(files);
}

/** The line of a record as [lon, lat] degrees. */
function lineOf({ line_udeg: udeg }) {
    const line = [];
    for (let i = 0, lon = 0, lat = 0; i < udeg.length; i += 2) line.push([(lon += udeg[i]) / 1e6, (lat += udeg[i + 1]) / 1e6]);
    return line;
}

/** The points file of a cover: one decimal of a pixel, and `null` and eye 0 for a point with no place. */
function pointsOf({ x, y, distance, visible }, credits) {
    const placed = i => [x[i], y[i], distance[i]].every(Number.isFinite), tenth = v => Math.round(v * 10) / 10;
    return {
        x: x.map((v, i) => (placed(i) ? tenth(v) : null)),
        y: y.map((v, i) => (placed(i) ? tenth(v) : null)),
        eye: distance.map((d, i) => (placed(i) ? Math.min(1 + Math.round(STEPS * Math.log2(Math.max(d, NEAREST_M) / NEAREST_M)), 127) | (visible[i] ? VISIBLE : 0) : 0)),
        credits,
    };
}

/** The cover of one line in the page: the view of `coverView`, then the still; `none` when no view holds the line. */
const shoot = (page, job) => page.evaluate(async ({ line, when, width, height, timeoutMs }) => {
    const { coverView, still } = window.trailviewStill, started = performance.now();
    try {
        const view = await coverView(line, { width, height, ...when });
        if (!view) return { none: true, seconds: (performance.now() - started) / 1000 };
        const { image, points, credits } = await still({ view, width, height, line }, { timeoutMs });
        if (image.type !== 'image/webp') return { failure: 'type', message: `the browser encoded ${image.type}, not WebP` };
        const url = await new Promise(resolve => {
            const reader = new FileReader();
            reader.onload = () => resolve(reader.result);
            reader.readAsDataURL(image);
        });
        const arrays = Object.fromEntries(Object.entries(points).map(([key, values]) => [key, Array.from(values)]));
        return { image: url.slice(url.indexOf(',') + 1), points: arrays, credits, seconds: (performance.now() - started) / 1000 };
    } catch (error) {
        return { failure: error.name, message: error.message };
    }
}, job);

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
        readJson(coverFile(out, record, 'json')).credits.forEach(credit => credits.add(credit));
        for (const cell of record.cells) (cells[cell] ??= {})[record.id] = keyOf(record);
    }
    const ids = Object.keys(cells).sort(), files = ids.map(cell => `${JSON.stringify({ format: 1, routes: cells[cell] })}\n`);
    const manifest = { format: 1, set: '', release, look: LOOK, width: WIDTH, height: HEIGHT, covers, credits: [...credits] };
    const hash = createHash('sha256');
    for (const file of files) hash.update(file);
    manifest.set = hash.update(JSON.stringify(manifest)).digest('hex').slice(0, 16);
    const dir = join(out, 'sets', manifest.set);
    mkdirSync(join(dir, 'cells'), { recursive: true });
    ids.forEach((cell, i) => writeFileSync(join(dir, 'cells', `${cell}.json`), files[i]));
    writeFileSync(join(dir, 'manifest.json'), `${JSON.stringify(manifest, null, 2)}\n`);
    return { set: manifest.set, manifest, cells: ids };
}

export async function render({ out = OUT, catalog, sample, only, gpu = false, fixture = false, fetch = globalThis.fetch }) {
    out = resolve(out);
    const active = catalog ? undefined : await productionRelease(fetch);
    const records = catalog ? routesOf([JSON.parse(readFileSync(catalog, 'utf8'))]) : await releaseRoutes(active, fetch);
    let chosen = only ? records.filter(record => only.includes(record.id)) : records;
    if (sample) chosen = chosen.filter((_, i) => i % Math.max(1, Math.floor(chosen.length / sample)) === 0).slice(0, sample);
    const todo = chosen.filter(record => !stateOf(out, record));
    console.log(`${records.length} routes, ${chosen.length} chosen, ${todo.length} to render.`);
    const when = { date: `${new Date().getFullYear()}-${DAY}`, time: TIME, weather: 'sunny' };
    const { origin, close } = await serve();
    const browser = await launch(gpu);
    let release = active?.id, covers = 0, none = 0, seconds = 0, bytes = 0;
    try {
        let page = await stillPage(browser, origin, { gpu, fixture });
        release ??= fixture ? 'fixture' : await page.evaluate(() => window.trailviewStill.release);
        if (!release) throw new Error('The page has no release id: build it against a catalogue, not a config.');
        for (const record of todo) {
            const job = { line: lineOf(record), when, width: WIDTH, height: HEIGHT, timeoutMs: gpu ? undefined : SOFTWARE_MS };
            let result = await shoot(page, job);
            if (result.failure === 'tiles-failed') {
                console.log(`${record.id}: ${result.message} Waiting ${RETRY_MS / 1000} s to try once more.`);
                await sleep(RETRY_MS);
                result = await shoot(page, job);
            }
            if (result.failure) throw new Error(`${record.id}: ${result.message} The next run starts again at this route.`);
            mkdirSync(coverDir(out, record), { recursive: true });
            seconds += result.seconds;
            if (result.none) {
                none++;
                writeFileSync(coverFile(out, record, 'none'), '');
                console.log(`${record.id}: no view, ${result.seconds.toFixed(1)} s`);
                continue;
            }
            const image = Buffer.from(result.image, 'base64');
            writeFileSync(coverFile(out, record, 'webp'), image);
            // Written last: a record without it renders again.
            writeFileSync(coverFile(out, record, 'json'), JSON.stringify(pointsOf(result.points, result.credits)));
            covers++;
            bytes += image.length;
            console.log(`${record.id}: ${result.seconds.toFixed(1)} s, ${Math.round(image.length / 1000)} kB`);
        }
        await page.close();
    } finally {
        await browser.close();
        close();
    }
    const done = covers + none, mean = done ? seconds / done : 0;
    console.log(`${covers} covers, ${none} routes with no view, ${mean.toFixed(1)} s each, ${(bytes / 1e6).toFixed(1)} MB.`);
    if (sample && done) {
        const all = Math.round(records.length * covers / done), cells = new Set(records.flatMap(record => record.cells)).size;
        console.log(`All ${records.length} routes: about ${(records.length * mean / 3600).toFixed(1)} h, ${(all * bytes / Math.max(covers, 1) / 1e6).toFixed(0)} MB, ${all} covers; an upload of ${2 * all + cells + 1} files at most.`);
    }
    const { set } = writeSet(out, records, release);
    console.log(`Set ${set} in ${join(out, 'sets', set)}.`);
}

/** `r2` put with the headers of an immutable key; after a 429 of the host it waits BUSY_MS and tries again, BUSY_TRIES times at most. */
async function put(r2, type, file, key) {
    for (let tries = 0; ; tries++) {
        try {
            return await r2(['put', '--immutable', '--content-type', type, '--cache-control', IMMUTABLE, file, key]);
        } catch (error) {
            if (tries >= BUSY_TRIES || !/\b429\b/.test(error.stderr ?? '')) throw error;
            console.log(`${key}: the host answers 429. Waiting ${BUSY_MS / 1000} s to try again.`);
            await sleep(BUSY_MS);
        }
    }
}

/** Runs `work` on each item, `count` at a time. */
async function pool(items, count, work) {
    let next = 0;
    await Promise.all(Array.from({ length: Math.min(count, items.length) }, async () => {
        while (next < items.length) await work(items[next++]);
    }));
}

/** The keys under `prefix` in the bucket, from one list. */
const listed = async (r2, prefix) => new Set(JSON.parse(await r2(['list', '--json', prefix])).objects.map(object => object.key));

/**
 * Makes the set of the covers in `out` for the production release, puts the covers that the bucket
 * does not hold and then the set, checks them on the public host, and gives the set id.
 */
export async function upload({ out = OUT, r2 = obcR2, fetch = globalThis.fetch }) {
    // `obc data` runs from the root of the repository, not from this directory.
    out = resolve(out);
    const release = await productionRelease(fetch), records = await releaseRoutes(release, fetch);
    const missing = records.filter(record => !stateOf(out, record)).map(record => record.id);
    if (missing.length) throw new Error(`These routes of the production release ${release.id} have no cover and no mark of no view: ${missing.join(', ')}. Run render first.`);
    const { set, cells } = writeSet(out, records, release.id), dir = join(out, 'sets', set);
    const covered = records.filter(record => stateOf(out, record) === 'cover'), held = await listed(r2, `${PREFIX}/covers/`);
    const filesOf = record => [['image/webp', 'webp'], ['application/json', 'json']].map(([type, ext]) => ({ type, file: coverFile(out, record, ext), key: `${PREFIX}/covers/${record.id}/${keyOf(record)}.${ext}` }));
    const files = covered.flatMap(filesOf);
    const sent = files.filter(({ key }) => !held.has(key));
    console.log(`${sent.length} of ${files.length} cover files to send.`);
    await pool(sent, PUTS, ({ type, file, key }) => put(r2, type, file, key));
    const setFiles = [...cells.map(cell => `cells/${cell}.json`), 'manifest.json'];
    await pool(setFiles, PUTS, name => put(r2, 'application/json', join(dir, name), `${PREFIX}/sets/${set}/${name}`));
    const step = Math.max(1, Math.floor(covered.length / CHECKED)), checked = covered.filter((_, i) => i % step === 0).slice(0, CHECKED).flatMap(filesOf);
    await checkHost([...setFiles.map(name => `${PREFIX}/sets/${set}/${name}`), ...checked.map(({ key }) => key)], fetch, `each file of set ${set}`);
    return set;
}

/**
 * Reads the cell files of an uploaded set from the bucket, checks with one list of the bucket that it
 * holds each cover that they name, then puts the manifest of the set at the live key. The bucket, not
 * the public host: the edge cache keeps a file for a year after it is deleted.
 */
export async function switchTo(set, { r2 = obcR2 } = {}) {
    if (!/^[0-9a-f]{16}$/.test(set ?? '')) throw new Error(`${set}: a set id is 16 hexadecimal digits.`);
    const held = await listed(r2, `${PREFIX}/`), root = `${PREFIX}/sets/${set}/`, key = `${root}manifest.json`;
    if (!held.has(key)) throw new Error(`The bucket has no ${key}.`);
    const dir = mkdtempSync(join(tmpdir(), 'route-pictures-')), file = join(dir, 'manifest.json');
    try {
        await r2(['get', key, file]);
        if (JSON.parse(readFileSync(file, 'utf8')).set !== set) throw new Error(`The manifest of set ${set} names another set.`);
        const cells = [...held].filter(name => name.startsWith(`${root}cells/`)), missing = [];
        await pool(cells, PUTS, async cell => {
            const copy = join(dir, cell.slice(cell.lastIndexOf('/') + 1));
            await r2(['get', cell, copy]);
            for (const [id, cover] of Object.entries(JSON.parse(readFileSync(copy, 'utf8')).routes)) {
                for (const ext of ['webp', 'json']) if (!held.has(`${PREFIX}/covers/${id}/${cover}.${ext}`)) missing.push(`${PREFIX}/covers/${id}/${cover}.${ext}`);
            }
        });
        if (missing.length) throw new Error(`The bucket does not hold each cover of set ${set}:\n${[...new Set(missing)].sort().join('\n')}`);
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
            catalog: { type: 'string' },
            sample: { type: 'string' },
            only: { type: 'string' },
            gpu: { type: 'boolean' },
            fixture: { type: 'boolean' },
        },
    });
    const out = values.out ?? OUT;
    if (command === 'switch') return console.log(`Set ${await switchTo(set)} is live.`);
    if (command === 'upload') return console.log(await upload({ out }));
    if (command === 'render') {
        const sample = values.sample === undefined ? undefined : Number(values.sample);
        if (sample !== undefined && !(Number.isInteger(sample) && sample > 0)) throw new Error('--sample takes a count of routes.');
        return render({ out, catalog: values.catalog, sample, only: values.only?.split(',').map(Number), gpu: values.gpu, fixture: values.fixture });
    }
    throw new Error('Usage: route-pictures.mjs render|upload|switch SET; see the head of this file.');
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
    await main().catch(error => {
        console.error(error.message);
        process.exitCode = 1;
    });
}
