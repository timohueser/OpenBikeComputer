import { execFile } from 'node:child_process';
import { mkdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { promisify } from 'node:util';
import { expect, test } from '@playwright/test';
import { HOST, manifestOf, readList, setOf, slotsOf, switchTo, times, upload } from './home-views.mjs';

const SCRIPT = new URL('./home-views.mjs', import.meta.url).pathname;
const CONFIG = JSON.parse(readFileSync(new URL('./fixture/config.json', import.meta.url), 'utf8'));
// Inside the fixture.
const ENGELBERG = { id: 'engelberg', name: 'Engelberg', region: 'Central Switzerland', view: '?lat=46.820500&lon=8.403000&heading=160.0&pitch=5.0&eye=1.6&date=2026-08-15&time=15:00&weather=sunny', night: true };
const TRUBSEE = { id: 'trubsee', name: 'Trübsee', region: 'Central Switzerland', view: '?lat=46.790000&lon=8.390000&heading=0.0&pitch=-10.0&alt=2500.0' };
const LIST = { date: '2026-07-15', views: [ENGELBERG, TRUBSEE] };
const CREDITS = { engelberg: ['terrain', 'sky'], trubsee: ['terrain', 'snow', 'sky'] };

/** A rendered set of LIST in `out`, with a few bytes as each image. */
function renderedSet(out) {
    for (const view of LIST.views) {
        const dir = join(out, LIST.date, view.id);
        mkdirSync(dir, { recursive: true });
        for (const slot of slotsOf(view)) writeFileSync(join(dir, `${slot}.webp`), `${view.id}/${slot}`);
        writeFileSync(join(dir, 'view.json'), JSON.stringify({ view: view.view, night: Boolean(view.night), credits: CREDITS[view.id] }));
    }
}

/** A fake of `obc data r2` that keeps each put, and of the public host that serves what was put. */
function bucket() {
    const puts = [], objects = new Map();
    const r2 = args => {
        const [file, key] = args.slice(-2);
        puts.push({ key, args: args.slice(0, -2) });
        objects.set(key, readFileSync(file, 'utf8'));
    };
    const fetch = async url => {
        const body = objects.get(url.slice(`${HOST}/`.length));
        return { status: body === undefined ? 404 : 200, text: async () => body };
    };
    return { puts, objects, r2, fetch };
}

test('the list is valid, and the sun gives each of its views its times', () => {
    const list = readList();
    for (const view of list.views) expect(Object.keys(times(view, list.date).slots)).toEqual(slotsOf(view));
});

test('a fixed list gives the manifest of the spec, with each credit one time', () => {
    expect(manifestOf(LIST, CREDITS)).toEqual({
        format: 1, set: '', date: '2026-07-15', width: 2560, height: 1440, credits: ['terrain', 'sky', 'snow'],
        views: [
            { id: 'engelberg', name: 'Engelberg', region: 'Central Switzerland', lat: 46.8205, lon: 8.403, sunrise: '05:50', sunset: '21:20', night: true },
            { id: 'trubsee', name: 'Trübsee', region: 'Central Switzerland', lat: 46.79, lon: 8.39, sunrise: '05:50', sunset: '21:20', night: false },
        ],
    });
    expect(times(ENGELBERG, LIST.date).slots).toEqual({ morning: '06:50', noon: '13:32', evening: '20:20', night: '23:00' });
});

test('a changed name gives a new set id', () => {
    const images = [Buffer.from('image')], set = setOf(manifestOf(LIST, CREDITS), images);
    expect(set).toMatch(/^[0-9a-f]{16}$/);
    const renamed = { ...LIST, views: [{ ...ENGELBERG, name: 'Engelberg village' }, TRUBSEE] };
    expect(setOf(manifestOf(renamed, CREDITS), images)).not.toBe(set);
});

test('a view north of the polar circle in June is refused, and so is a night with no darkness', () => {
    expect(() => times({ id: 'tromso', view: '?lat=69.65&lon=18.95' }, '2026-06-15')).toThrow('the sun does not rise or does not set');
    expect(() => times({ id: 'oslo', view: '?lat=59.91&lon=10.75', night: true }, '2026-06-15')).toThrow('has no night');
});

test('upload refuses a set with a missing image, else puts the images and the manifest under the set id; switch puts that manifest live', async () => {
    const out = test.info().outputPath('set'), fake = bucket();
    renderedSet(out);
    writeFileSync(join(out, LIST.date, 'trubsee', 'view.json'), JSON.stringify({ view: '?lat=1&lon=1', night: false, credits: [] }));
    await expect(upload({ list: LIST, out, ...fake })).rejects.toThrow('trubsee');
    expect(fake.puts).toEqual([]);

    renderedSet(out);
    const set = await upload({ list: LIST, out, ...fake });
    const keys = ['engelberg/morning', 'engelberg/noon', 'engelberg/evening', 'engelberg/night', 'trubsee/morning', 'trubsee/noon', 'trubsee/evening'];
    expect(fake.puts.map(({ key }) => key)).toEqual([...keys.map(key => `home-views/${set}/${key}.webp`), `home-views/${set}/manifest.json`]);
    expect(fake.puts.every(({ args }) => args.includes('--immutable'))).toBe(true);
    expect(JSON.parse(fake.objects.get(`home-views/${set}/manifest.json`))).toEqual({ ...manifestOf(LIST, CREDITS), set });

    await switchTo(set, fake);
    expect(fake.puts.at(-1)).toEqual({ key: 'home-views/manifest.json', args: ['put', '--content-type', 'application/json', '--cache-control', 'public, max-age=60, must-revalidate'] });
    expect(fake.objects.get('home-views/manifest.json')).toBe(fake.objects.get(`home-views/${set}/manifest.json`));
    fake.objects.delete(`home-views/${set}/trubsee/noon.webp`);
    await expect(switchTo(set, fake)).rejects.toThrow('trubsee/noon.webp: 404');
});

test('render --fixture --preview writes the images of a view and their credits, and a second run skips them', { tag: '@gpu' }, async () => {
    const out = test.info().outputPath('render'), list = join(out, 'list.json');
    mkdirSync(out, { recursive: true });
    writeFileSync(list, JSON.stringify({ date: '2026-08-15', views: [ENGELBERG] }));
    const run = () => promisify(execFile)(process.execPath, [SCRIPT, 'render', '--fixture', '--preview', '--gpu', '--list', list, '--out', out]);
    const first = await run().catch(error => error);
    test.skip(/no GPU for stills/.test(first.stderr), 'This browser renders stills in software only.');
    expect(first.code, first.stderr).toBeUndefined();
    const dir = join(out, 'preview', '2026-08-15', 'engelberg');
    expect(first.stdout).toContain('engelberg/night at 23:00');
    const written = slotsOf(ENGELBERG).map(slot => statSync(join(dir, `${slot}.webp`)).mtimeMs);
    const { credits } = JSON.parse(readFileSync(join(dir, 'view.json'), 'utf8'));
    expect(credits).toContain(CONFIG.terrain_attribution);

    const second = await run();
    expect(second.stdout).toContain('engelberg: skipped');
    expect(slotsOf(ENGELBERG).map(slot => statSync(join(dir, `${slot}.webp`)).mtimeMs)).toEqual(written);
});
