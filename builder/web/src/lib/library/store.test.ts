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
        await Promise.all([first.saveRoute({ ...saved, updatedAt: 2 }), first.saveRoute({ ...saved, updatedAt: 3 })]);
        expect((await first.routes())[0]).toMatchObject({ updatedAt: 3, revision: 3 });
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

    it('keeps a rename over a save that was queued before it', async () => {
        const library = new Library(new IDBFactory()), saved = await library.saveRoute(route('a'));
        const [renamed, later] = await Promise.all([library.renameRoute('a', 'Lake loop'), library.saveRoute({ ...saved, updatedAt: 5 })]);
        expect(renamed).toMatchObject({ name: 'Lake loop', revision: 2 });
        expect(later).toMatchObject({ name: 'Lake loop', revision: 3, updatedAt: 5 });
        expect(await library.route('a')).toMatchObject({ name: 'Lake loop', revision: 3 });
        await expect(library.renameRoute('gone', 'x')).rejects.toThrow('not in the library');
        await library.renameRoute('a', 'Lake round');
        // The copy of the route still has revision 1: the tab knows the revisions of its own save and rename.
        await library.removeRoute('a', saved.revision);
        expect(await library.route('a')).toBeUndefined();
        await library.close();
    });

    it('does not hide a save of another tab behind a rename', async () => {
        const factory = new IDBFactory(), stale = new Library(factory), other = new Library(factory);
        const saved = await stale.saveRoute(route('a'));
        await other.saveRoute({ ...(await other.route('a'))!, trip: { points: [], edited: true } });
        expect(await stale.renameRoute('a', 'Lake loop')).toMatchObject({ name: 'Lake loop', revision: 3 });
        await expect(stale.saveRoute({ ...saved, updatedAt: 2 })).rejects.toThrow('another tab');
        await expect(stale.removeRoute('a', saved.revision)).rejects.toThrow('changed');
        expect(await other.route('a')).toMatchObject({ name: 'Lake loop', trip: { edited: true } });
        await stale.close(); await other.close();
    });

    it('changes a state value in one transaction, and a removed route stops waiting and leaves the device', async () => {
        const library = new Library(new IDBFactory()), heard: string[] = [];
        library.onChange((store, key) => heard.push(`${store}:${key}`));
        await library.saveRoute(route('a'));
        const add = (id: string) => library.changeState('waiting', ids => [...ids ?? [], id]);
        await Promise.all([add('a'), add('b')]);
        expect(await library.state('waiting')).toEqual(['a', 'b']);
        await library.removeRoute('a');
        expect(await library.state('waiting')).toEqual(['b']);
        expect(heard).toEqual(['routes:a', 'state:waiting', 'state:waiting', 'routes:a', 'state:waiting']);
        // Only a route on the device has to leave it.
        expect(await library.state('leaving')).toBeUndefined();
        await library.saveRoute(route('c'));
        await library.setState('sent', { c: { device: '1:7', key: 'k' } });
        await library.removeRoute('c');
        expect(await library.state('leaving')).toEqual(['c']);
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

    it('clears all stores after the writes before it, and then refuses each write', async () => {
        const factory = new IDBFactory(), library = new Library(factory);
        await library.saveRoute(route('a'), [{ id: 'v' }]);
        await library.addRide(ride('20260801T070000Z'), new Blob(['ride']));
        await library.saveCover({ id: 'a', lineKey: 'k', state: 'none', tries: 0 });
        await library.setState('active', 'a');
        const before = library.saveRoute(route('b'));
        await library.clear();
        await expect(before).resolves.toMatchObject({ id: 'b' });
        await expect(library.saveRoute(route('c'))).rejects.toThrow('deleted');
        const fresh = new Library(factory), contents = await fresh.contents();
        expect([contents.routes, contents.versions, contents.rides, contents.files.size, contents.state, await fresh.cover('a')])
            .toEqual([[], {}, [], 0, {}, undefined]);
        await library.close(); await fresh.close();
    });
});
