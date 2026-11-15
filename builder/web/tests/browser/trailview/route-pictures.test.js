import { execFile } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { promisify } from 'node:util';
import { expect, test } from '@playwright/test';
import { CATALOG, HOST } from './stills.mjs';
import { FIXTURE_ROUTES, LOOK, checkRelease, gridCells, keyOf, patiently, pointsOf, sampleOf, switchTo, upload } from './route-pictures.mjs';
import { pack } from '../../../src/lib/covers/line.ts';

const SCRIPT = new URL('./route-pictures.mjs', import.meta.url).pathname;
const CONFIG = JSON.parse(readFileSync(new URL('./fixture/config.json', import.meta.url), 'utf8'));
const [VALLEY] = JSON.parse(readFileSync(FIXTURE_ROUTES, 'utf8')).routes;
const NO_VIEW = { ...VALLEY, id: 1002, cells: ['9-268-180'], line_udeg: [8450000, 46800000, 1000, 0] };
const LONG = { id: 1003, kind: 'hiking', name: 'Long', cells: ['9-267-180'], stages: [1001] };
const RELEASE = { id: 'release-1', bounds: CONFIG.bounds, routes: 'https://tiles.test/releases/release-1/routes/tiles/{cell}.json' };

/** The cover of VALLEY and the mark of NO_VIEW in `out`, with a few bytes as the image. */
function rendered(out) {
    const file = ext => join(out, 'covers', String(VALLEY.id), `${keyOf(VALLEY)}.${ext}`);
    mkdirSync(dirname(file('webp')), { recursive: true });
    writeFileSync(file('webp'), 'image');
    writeFileSync(file('json'), JSON.stringify({ x: [], y: [], eye: [], credits: ['terrain'] }));
    const none = join(out, 'covers', String(NO_VIEW.id), `${keyOf(NO_VIEW)}.none`);
    mkdirSync(dirname(none), { recursive: true });
    writeFileSync(none, '');
}

/**
 * A fake of `obc data r2` (list, get, put) and of the hosts: the catalogue, the cell files of RELEASE
 * and what was put. It refuses a key or folder with an empty part, as `check_key` does, and keeps each
 * put when it ends. A cell file takes longer to put than another file.
 */
function bucket() {
    const puts = [], objects = new Map();
    const r2 = async ([action, ...args]) => {
        const key = action === 'put' ? args.at(-1) : action === 'get' ? args[0] : args.at(-1);
        if (key.split('/').some(part => !part)) throw new Error(`${key} does not name an object or folder inside the bucket`);
        if (action === 'list') return JSON.stringify({ objects: [...objects.keys()].filter(name => name.startsWith(`${key}/`)).map(name => ({ key: name })) });
        if (action === 'get') return writeFileSync(args[1], objects.get(key));
        const body = readFileSync(args.at(-2), 'utf8');
        await new Promise(resolve => setTimeout(resolve, key.includes('/cells/') ? 20 : 0));
        puts.push({ key, args: [action, ...args.slice(0, -2)] });
        objects.set(key, body);
    };
    const fetch = async url => {
        const cell = /routes\/tiles\/(.+)\.json$/.exec(url)?.[1];
        const body = url === CATALOG ? JSON.stringify({ format: 1, active: RELEASE })
            : cell ? JSON.stringify({ format: 1, routes: [VALLEY, NO_VIEW, LONG].filter(record => record.cells.includes(cell)) })
                : objects.get(url.slice(`${HOST}/`.length));
        return { status: body === undefined ? 404 : 200, text: async () => body };
    };
    return { puts, objects, r2, fetch };
}

test('the grid cells of the fixture are the zoom 9 cells that its bounds overlap', () => {
    expect(gridCells(CONFIG.bounds)).toEqual(['9-267-180', '9-268-180']);
    // Bounds on the edges of one cell, a little off in the last bits as from another builder.
    const latitude = row => Math.atan(Math.sinh(Math.PI * (1 - 2 * row / 512))) * 180 / Math.PI;
    expect(gridCells([267 / 512 * 360 - 180, latitude(181) * (1 - 1e-15), 268 / 512 * 360 - 180, latitude(180) * (1 + 1e-15)])).toEqual(['9-267-180']);
});

test('a sample spreads over the routes, and takes each route when it asks for more', () => {
    const records = Array.from({ length: 59 }, (_, i) => i);
    expect(sampleOf(records, 30)).toEqual(Array.from({ length: 30 }, (_, i) => Math.floor(i * 59 / 30)));
    expect(sampleOf(records, 30).at(-1)).toBe(57);
    expect(sampleOf(records, 80)).toEqual(records);
});

test('after a 429 of the host a request waits and tries again, 5 times at most', async () => {
    const waits = [], busy = Object.assign(new Error('busy'), { stderr: 'HTTP 429 Too Many Requests' });
    let calls = 0;
    expect(await patiently('key', async () => {
        if (++calls < 3) throw busy;
        return 'sent';
    }, async ms => waits.push(ms))).toBe('sent');
    expect(waits).toEqual([60_000, 60_000]);
    calls = 0;
    await expect(patiently('key', async () => { calls++; throw busy; }, async () => {})).rejects.toThrow('busy');
    expect(calls).toBe(6);
    await expect(patiently('key', async () => { calls++; throw new Error('denied'); }, async () => {})).rejects.toThrow('denied');
});

test('the points file has the eye code of pack, and no place where pack has none', () => {
    const still = { x: Float32Array.of(1.04, -20.26, 970, 3, NaN, 7), y: Float32Array.of(2, 3, 4, 5, NaN, 8), distance: Float32Array.of(5, 10, 1000, 1e9, NaN, 123.4), visible: Uint8Array.of(1, 0, 1, 0, 0, 1) };
    const points = pointsOf(still, ['terrain']);
    expect(points.eye).toEqual(Array.from(pack(still).eye));
    expect(points).toEqual({ x: [1, -20.3, 970, 3, null, 7], y: [2, 3, 4, 5, null, 8], eye: points.eye, credits: ['terrain'] });
});

test('render refuses a still page of another release than production', () => {
    expect(() => checkRelease(undefined, RELEASE.id)).toThrow('npm run build:trailview');
    expect(() => checkRelease('release-0', RELEASE.id)).toThrow('release release-0, not the production release release-1');
    checkRelease(RELEASE.id, RELEASE.id);
});

test('upload refuses a route with no cover and no mark, else sends the covers that the bucket lacks and then the set; switch makes it live', async () => {
    const out = test.info().outputPath('out'), fake = bucket();
    await expect(upload({ out, ...fake })).rejects.toThrow(`release ${RELEASE.id} have no cover and no mark of no view: 1001, 1002.`);
    await expect(upload({ out, without: [1002], ...fake })).rejects.toThrow('no mark of no view: 1001.');
    expect(fake.puts).toEqual([]);

    rendered(out);
    const cover = `route-pictures/covers/1001/${keyOf(VALLEY)}`;
    fake.objects.set(`${cover}.json`, 'held');
    const set = await upload({ out, ...fake });
    // The manifest goes last, after the slower cell file.
    expect(fake.puts.map(({ key }) => key)).toEqual([`${cover}.webp`, `route-pictures/sets/${set}/cells/9-267-180.json`, `route-pictures/sets/${set}/manifest.json`]);
    expect(fake.puts.every(({ args }) => args.includes('--immutable') && args.includes('public, max-age=31536000, immutable'))).toBe(true);
    expect(JSON.parse(fake.objects.get(`route-pictures/sets/${set}/cells/9-267-180.json`))).toEqual({ format: 1, routes: { 1001: keyOf(VALLEY) } });
    expect(JSON.parse(fake.objects.get(`route-pictures/sets/${set}/manifest.json`))).toEqual({ format: 1, set, release: RELEASE.id, look: LOOK, width: 960, height: 640, covers: 1, credits: ['terrain'] });

    await switchTo(set, fake);
    expect(fake.puts.at(-1)).toMatchObject({ key: 'route-pictures/manifest.json', args: ['put', '--content-type', 'application/json', '--cache-control', 'public, max-age=60, must-revalidate'] });
    const puts = fake.puts.length;
    fake.objects.delete(`${cover}.webp`);
    await expect(switchTo(set, fake)).rejects.toThrow(`${cover}.webp`);
    await expect(switchTo('0123456789abcdef', fake)).rejects.toThrow('The bucket has no route-pictures/sets/0123456789abcdef/manifest.json');
    expect(fake.puts.length).toBe(puts);
});

test('render --fixture writes the cover of a route, its points, the cell file and the manifest, and a second run renders nothing', { tag: '@gpu' }, async () => {
    const out = test.info().outputPath('render');
    const run = () => promisify(execFile)(process.execPath, [SCRIPT, 'render', '--fixture', '--gpu', '--out', out]);
    const first = await run().catch(error => error);
    test.skip(/no GPU for stills/.test(first.stderr), 'This browser renders stills in software only.');
    expect(first.code, first.stderr).toBeUndefined();
    const set = /Set ([0-9a-f]{16})/.exec(first.stdout)[1], manifest = JSON.parse(readFileSync(join(out, 'sets', set, 'manifest.json'), 'utf8'));
    expect(manifest).toMatchObject({ format: 1, set, release: 'fixture', width: 960, height: 640, covers: 1 });
    expect(manifest.look).toMatch(/^1\|\d\d-\d\d\|\d\d:\d\d\|960\|640$/);
    expect(manifest.credits).toContain(CONFIG.terrain_attribution);
    const key = createHash('sha256').update(`${manifest.look}|${VALLEY.line_udeg.join(',')}`).digest('hex').slice(0, 16);
    expect(JSON.parse(readFileSync(join(out, 'sets', set, 'cells', '9-267-180.json'), 'utf8'))).toEqual({ format: 1, routes: { [VALLEY.id]: key } });

    const image = join(out, 'covers', String(VALLEY.id), `${key}.webp`), written = statSync(image).mtimeMs;
    expect(readFileSync(image).subarray(8, 12).toString()).toBe('WEBP');
    const points = JSON.parse(readFileSync(join(out, 'covers', String(VALLEY.id), `${key}.json`), 'utf8'));
    for (const axis of ['x', 'y', 'eye']) expect(points[axis]).toHaveLength(VALLEY.line_udeg.length / 2);
    // The cover view holds the route, so each vertex has a place inside the image.
    points.x.forEach((x, i) => {
        expect(points.eye[i], `vertex ${i}`).toBeGreaterThan(0);
        expect(x >= 0 && x < 960 && points.y[i] >= 0 && points.y[i] < 640, `vertex ${i}`).toBe(true);
    });

    const second = await run();
    expect(second.stdout).toContain('0 to render');
    expect(second.stdout).toContain(`Set ${set}`);
    expect(statSync(image).mtimeMs).toBe(written);
});
