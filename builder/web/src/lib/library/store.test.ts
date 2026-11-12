import { IDBFactory } from 'fake-indexeddb';
import { describe, expect, it } from 'vitest';
import type { RideRecord, RouteRecord } from './records';
import { Library } from './store';
import { summarize } from './summary';

const route = (id: string): RouteRecord => ({ id, revision: 0, name: id, updatedAt: 1, summary: null, trip: { points: [] } });

const ride = (key: string, device?: string): RideRecord => ({
    key, name: key, source: device ? 'device' : 'file', format: 'gpx', maxHr: 180, ftp: null, ...device && { device },
    summary: summarize([[8, 47, 400], [8.01, 47, 420]], { lengthM: 760, climbM: 20, seconds: 300, movingS: 280, start: 1_700_000_000, bike: 'gravel' }),
});

describe('library store', () => {
    it('saves routes in order, refuses an unseen edit from another tab, and removes the versions and the active pointer', async () => {
        const factory = new IDBFactory(), first = new Library(factory), second = new Library(factory);
        const saved = await first.saveRoute(route('a'), [{ id: 'v' }]);
        await first.setState('active', saved.id);
        const stale = (await second.route('a'))!;
        await Promise.all([first.saveRoute({ ...saved, name: 'First' }), first.saveRoute({ ...saved, name: 'Last' })]);
        expect((await first.routes())[0]).toMatchObject({ name: 'Last', revision: 3 });
        expect(await first.versions('a')).toEqual([{ id: 'v' }]);
        await expect(second.saveRoute({ ...stale, name: 'Unseen' })).rejects.toThrow('another tab');
        await expect(second.removeRoute('a', stale.revision)).rejects.toThrow('changed');
        await second.removeRoute('a', (await second.route('a'))!.revision);
        expect([await first.routes(), await first.versions('a'), await first.state('active')]).toEqual([[], [], null]);
        await first.close(); await second.close();
    });

    it('adds a route only when its id is not stored', async () => {
        const library = new Library(new IDBFactory());
        expect(await library.addRoute(route('file-a'))).toBe(true);
        expect(await library.addRoute({ ...route('file-a'), name: 'Other' })).toBe(false);
        expect((await library.routes()).map(r => r.name)).toEqual(['file-a']);
        await library.close();
    });

    it('adds a ride once, and refuses a stored or a deleted key but keeps its device id', async () => {
        const library = new Library(new IDBFactory()), file = new Blob(['ride']);
        expect(await library.addRide(ride('20260801T070000Z'), file)).toBe(true);
        expect(await library.addRide(ride('20260801T070000Z', '1:5'), new Blob(['other']))).toBe(false);
        expect(await (await library.rideFile('20260801T070000Z'))!.text()).toBe('ride');
        expect((await library.ride('20260801T070000Z'))!.device).toBe('1:5');

        expect(await library.addRide(ride('20260802T070000Z'), file)).toBe(true);
        await library.removeRide('20260802T070000Z');
        expect(await library.addRide(ride('20260802T070000Z', '1:6'), file)).toBe(false);
        expect(await library.ride('20260802T070000Z')).toBeUndefined();
        expect(await library.state('deleted')).toEqual([{ key: '20260802T070000Z', device: '1:6' }]);
        expect(await library.deviceRides()).toEqual(new Set(['1:5', '1:6']));
        await library.close();
    });
});
