import { IDBFactory } from 'fake-indexeddb';
import { describe, expect, it } from 'vitest';
import { loadBackup, saveBackup } from './backup';
import type { RideRecord, RouteRecord, RouteSummary } from './records';
import { Library } from './store';
import { summarize } from './summary';
import { unzip, zip } from './zip';

const route: RouteRecord = { id: 'r1', revision: 0, name: 'Alps', updatedAt: 5, open: 'v1',
    summary: summarize<RouteSummary>([[8, 47, 400], [8.1, 47.1, 900]], { lengthM: 13_000, climbM: 500, seconds: 3600, days: 1, bike: 'touring' }),
    trip: { points: [], routeOrder: [], days: 1 } };

const ride = (key: string): RideRecord => ({ key, name: key, source: 'device', format: 'obcride', maxHr: 182, ftp: 250, device: `7:${key.slice(6, 8)}`,
    summary: summarize([[8, 47, null], [8.01, 47, null]], { lengthM: 760, climbM: 0, seconds: 300, movingS: 280, start: 1_785_000_000, bike: 'gravel' }) });

/** A library with one route, two rides and a deleted ride. */
async function filled(): Promise<Library> {
    const library = new Library(new IDBFactory());
    await library.saveRoute(route, [{ id: 'v1', at: '2026-08-01T07:00:00.000Z', trip: route.trip }]);
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
        expect(await loadBackup(target, backup)).toEqual({ routes: 1, rides: 2, settings: { units: 'km' } });
        expect(await target.contents()).toEqual(await source.contents());
        expect(await (await target.rideFile('20260802T070000Z'))!.text()).toBe('bytes of 20260802T070000Z');
        expect(await loadBackup(target, backup)).toEqual({ routes: 0, rides: 0 });
        expect((await target.routes())[0].revision).toBe(1);
    });

    it('changes nothing for a damaged file or a file of another version', async () => {
        const backup = await saveBackup(await filled(), {}), target = new Library(new IDBFactory());
        const bytes = new Uint8Array(await backup.arrayBuffer());
        bytes[45] ^= 0xff;
        const files = [new Blob([bytes]), backup.slice(0, backup.size / 2), await edited(backup, json => ({ ...json, version: 2 })),
            await edited(backup, json => ({ ...json, rides: [...json.rides as RideRecord[], ride('20260809T070000Z')] })),
            await edited(backup, json => ({ ...json, state: { nextMap: { regions: [], boxes: [{ minLat: 46.5 }], light: 'default', dark: 'dusk', bytes: 1 } } })),
            // A box with no height.
            await edited(backup, json => ({ ...json, state: { nextMap: { regions: [], boxes: [{ minLat: 46_000_000, minLon: 8_000_000, maxLat: 46_000_000, maxLon: 9_000_000 }], light: 'default', dark: 'dusk', bytes: 1 } } }))];
        for (const file of files) await expect(loadBackup(target, file)).rejects.toThrow();
        expect(await target.contents()).toEqual({ routes: [], versions: {}, rides: [], files: new Map(), state: {} });
    });

    it('does not bring back a ride that the rider deleted', async () => {
        const backup = await saveBackup(await filled(), {}), target = new Library(new IDBFactory());
        await target.addRide(ride('20260801T070000Z'), new Blob(['kept']));
        await target.removeRide('20260801T070000Z');
        expect(await loadBackup(target, backup)).toEqual({ routes: 1, rides: 1 });
        expect((await target.rides()).map(r => r.key)).toEqual(['20260802T070000Z']);
    });
});
