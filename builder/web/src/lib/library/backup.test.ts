import { IDBFactory } from 'fake-indexeddb';
import { describe, expect, it } from 'vitest';
import { loadBackup, saveBackup } from './backup';
import { isPhotoEntry, type RideRecord, type RouteRecord, type RouteSummary } from './records';
import { Library } from './store';
import { summarize } from './summary';
import { TooLarge, unzip, zip } from './zip';

const route: RouteRecord = { id: 'r1', name: 'Alps', updatedAt: 5,
    summary: summarize<RouteSummary>([[8, 47, 400], [8.1, 47.1, 900]], { lengthM: 13_000, climbM: 500, seconds: 3600, days: 1, bike: 'touring' }),
    trip: { points: [], routeOrder: [], days: 1 } };

const ride = (key: string): RideRecord => ({ key, name: key, source: 'device', format: 'obcride', maxHr: 182, ftp: 250, device: `7:${key.slice(6, 8)}`,
    summary: summarize([[8, 47, null], [8.01, 47, null]], { lengthM: 760, climbM: 0, seconds: 300, movingS: 280, start: 1_785_000_000, bike: 'gravel' }) });

/** A library with one route, two rides and a deleted ride. */
async function filled(): Promise<Library> {
    const library = new Library(new IDBFactory());
    await library.saveRoute(route);
    await library.setState('active', 'r1');
    await library.setState('nextMap', { regions: ['europe/switzerland'], boxes: [{ minLat: 46_000_000, minLon: 8_000_000, maxLat: 46_500_000, maxLon: 9_000_000 }], light: 'default', dark: 'dusk', bytes: 870_000_000 });
    for (const key of ['20260801T070000Z', '20260802T070000Z', '20260803T070000Z']) await library.addRide(ride(key), new Blob([`bytes of ${key}`]));
    await library.removeRide('20260803T070000Z');
    return library;
}

/** The backup with `library.json` replaced by `edit` of it. */
async function edited(backup: Blob, edit: (json: Record<string, unknown>) => unknown): Promise<Blob> {
    const entries = await unzip(backup), json = JSON.parse(await entries.get('library.json')!.text());
    return zip([...entries].map(([name, data]) => ({ name, data: name === 'library.json' ? new Blob([JSON.stringify(edit(json))]) : data })));
}

describe('library backup', () => {
    it('loads into an empty library with equal records and the settings, and a second load adds nothing', async () => {
        const source = await filled(), backup = await saveBackup(source, { units: 'km' });
        const target = new Library(new IDBFactory());
        expect(await loadBackup(target, backup)).toEqual({ routes: 1, rides: 2, photos: 0, settings: { units: 'km' } });
        expect(await target.contents()).toEqual(await source.contents());
        expect(await (await target.rideFile('20260802T070000Z'))!.text()).toBe('bytes of 20260802T070000Z');
        expect(await loadBackup(target, backup)).toEqual({ routes: 0, rides: 0, photos: 0 });
        // A file with no photos has no `photos` field.
        const older = await edited(backup, ({ photos: _, ...json }) => json);
        expect(await loadBackup(new Library(new IDBFactory()), older)).toMatchObject({ routes: 1, rides: 2, photos: 0 });
        expect(JSON.parse(await (await unzip(backup)).get('library.json')!.text())).not.toHaveProperty('versions');
        // A reader ignores the saved versions of an older file.
        const versions = await edited(backup, json => ({ ...json, routes: [{ ...route, open: 'v1' }],
            versions: { r1: [{ id: 'v1', at: '2026-08-01T07:00:00.000Z', trip: route.trip }] } }));
        const merged = new Library(new IDBFactory());
        expect(await loadBackup(merged, versions)).toMatchObject({ routes: 1, rides: 2, photos: 0 });
        expect(await merged.routes()).toEqual([route]);
        expect(JSON.parse(await (await unzip(await saveBackup(merged, {}))).get('library.json')!.text()).routes[0]).not.toHaveProperty('open');
        // A reader ignores a field of the envelope and an entry that it does not know.
        const later = await zip([...(await unzip(await edited(backup, json => ({ ...json, later: [1] }))))].map(([name, data]) => ({ name, data }))
            .concat({ name: 'later/a.bin', data: new Blob(['later']) }));
        expect(await loadBackup(new Library(new IDBFactory()), later)).toMatchObject({ routes: 1, rides: 2, photos: 0 });
    });

    it('brings back each photo with its copy and a new small picture, one time, also for a ride that the library has', async () => {
        const source = await filled(), key = '20260801T070000Z';
        await source.addPhoto({ id: `${key}-a1`, ride: key, time: 1_785_000_100, image: new Blob(['copy a1'], { type: 'image/jpeg' }), small: new Blob(['small a1']) });
        await source.addPhoto({ id: `${key}-b2`, ride: key, time: null, place: [8.005, 47], image: new Blob(['copy b2']), small: new Blob(['small b2']) });
        const backup = await saveBackup(source, {}), small = async (image: Blob) => new Blob([`small of ${await image.text()}`]);
        expect([...(await unzip(backup)).keys()].filter(name => name.startsWith('photos/'))).toEqual([`photos/${key}-a1.jpg`, `photos/${key}-b2.jpg`]);

        const target = new Library(new IDBFactory());
        expect(await loadBackup(target, backup, small)).toMatchObject({ rides: 2, photos: 2 });
        const read = async () => Promise.all((await target.photos(key)).map(async ({ image, small, ...record }) => [record, image.type, await image.text(), await small.text()]));
        const loaded = [[{ id: `${key}-a1`, ride: key, time: 1_785_000_100 }, 'image/jpeg', 'copy a1', 'small of copy a1'],
            [{ id: `${key}-b2`, ride: key, time: null, place: [8.005, 47] }, 'image/jpeg', 'copy b2', 'small of copy b2']];
        expect(await read()).toEqual(loaded);
        expect(await loadBackup(target, backup, small)).toEqual({ routes: 0, rides: 0, photos: 0 });
        await target.removePhoto(`${key}-b2`);
        expect(await loadBackup(target, backup, small)).toEqual({ routes: 0, rides: 0, photos: 1 });
        expect(await read()).toEqual(loaded);

        // A backup with no photos holds no copy, and a load of it adds none.
        const bare = await saveBackup(source, {}, false);
        expect([...(await unzip(bare)).keys()].some(name => name.startsWith('photos/'))).toBe(false);
        expect(await loadBackup(new Library(new IDBFactory()), bare, small)).toMatchObject({ rides: 2, photos: 0 });
    });

    it('takes a photo record with a time or with a place, and none with both or with neither', () => {
        const record = { id: 'p1', ride: '20260801T070000Z' };
        expect([{ ...record, time: 1 }, { ...record, time: null, place: [8, 47] }].every(isPhotoEntry)).toBe(true);
        expect([{ ...record, time: 1, place: [8, 47] }, { ...record, time: null }].some(isPhotoEntry)).toBe(false);
    });

    it('refuses entries of 4 GiB or more before it reads one', async () => {
        const huge = { size: 2 ** 32 - 200, stream: () => { throw new Error('read'); } } as unknown as Blob;
        await expect(zip([{ name: 'library.json', data: new Blob(['{}']) }, { name: 'photos/a.jpg', data: huge }])).rejects.toBeInstanceOf(TooLarge);
    });

    it('changes nothing for a damaged file or a file of another version', async () => {
        const backup = await saveBackup(await filled(), {}), target = new Library(new IDBFactory());
        const bytes = new Uint8Array(await backup.arrayBuffer());
        bytes[45] ^= 0xff;
        const files = [new Blob([bytes]), backup.slice(0, backup.size / 2), await edited(backup, json => ({ ...json, version: 2 })),
            await edited(backup, json => ({ ...json, rides: [...json.rides as RideRecord[], ride('20260809T070000Z')] })),
            // A photo with no copy in the file.
            await edited(backup, json => ({ ...json, photos: [{ id: 'p1', ride: '20260801T070000Z', time: 1 }] })),
            await edited(backup, json => ({ ...json, state: { nextMap: { regions: [], boxes: [{ minLat: 46.5 }], light: 'default', dark: 'dusk', bytes: 1 } } })),
            // A box with no height.
            await edited(backup, json => ({ ...json, state: { nextMap: { regions: [], boxes: [{ minLat: 46_000_000, minLon: 8_000_000, maxLat: 46_000_000, maxLon: 9_000_000 }], light: 'default', dark: 'dusk', bytes: 1 } } }))];
        for (const file of files) await expect(loadBackup(target, file)).rejects.toThrow();
        expect(await target.contents()).toEqual({ routes: [], rides: [], files: new Map(), state: {} });
    });

    it('does not bring back a ride that the rider deleted', async () => {
        const backup = await saveBackup(await filled(), {}), target = new Library(new IDBFactory());
        await target.addRide(ride('20260801T070000Z'), new Blob(['kept']));
        await target.removeRide('20260801T070000Z');
        expect(await loadBackup(target, backup)).toEqual({ routes: 1, rides: 1, photos: 0 });
        expect((await target.rides()).map(r => r.key)).toEqual(['20260802T070000Z']);
    });
});
