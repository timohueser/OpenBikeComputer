import { IDBFactory } from 'fake-indexeddb';
import { describe, expect, it } from 'vitest';
import type { PhotoRecord, RideRecord, RouteRecord, RouteSummary } from './records';
import { Library } from './store';
import { summarize } from './summary';

const route = (id: string): RouteRecord => ({ id, name: id, updatedAt: 1, summary: null, trip: { points: [] } });

const ride = (key: string, device?: string): RideRecord => ({
    key, name: key, source: device ? 'device' : 'file', format: 'gpx', maxHr: 180, ftp: null, ...device && { device },
    summary: summarize([[8, 47, 400], [8.01, 47, 420]], { lengthM: 760, climbM: 20, seconds: 300, movingS: 280, start: 1_700_000_000, bike: 'gravel' }),
});

const photo = (id: string, key: string): PhotoRecord => ({ id, ride: key, time: 1, image: new Blob([`copy of ${id}`]), small: new Blob(['small']) });

describe('library store', () => {
    it('saves routes in order, lets the later save of two tabs win, and deletes the draft and the active pointer with the route', async () => {
        const factory = new IDBFactory(), first = new Library(factory), second = new Library(factory);
        const saved = await first.saveRoute(route('a'));
        await first.setState('active', saved.id);
        await Promise.all([first.saveRoute({ ...saved, updatedAt: 2 }), first.saveRoute({ ...saved, updatedAt: 3 })]);
        expect((await first.routes())[0]).toMatchObject({ updatedAt: 3 });
        await second.saveRoute({ ...saved, updatedAt: 4, trip: { points: [], later: true } });
        expect(await first.route('a')).toMatchObject({ updatedAt: 4, trip: { later: true } });
        await first.saveDraft({ id: 'a', updatedAt: 5, trip: { points: [] } });
        await second.removeRoute('a');
        expect([await first.routes(), await first.draft('a'), await first.state('active')]).toEqual([[], undefined, null]);
        await first.close(); await second.close();
    });

    it('adds a route only when its id is not stored', async () => {
        const library = new Library(new IDBFactory());
        expect(await library.addRoute(route('file-a'))).toBe(true);
        expect(await library.addRoute({ ...route('file-a'), name: 'Other' })).toBe(false);
        expect((await library.routes()).map(r => r.name)).toEqual(['file-a']);
        await library.close();
    });

    it('keeps a rename over a save that was queued before it, and saves a route again that another tab deleted', async () => {
        const library = new Library(new IDBFactory()), saved = await library.saveRoute(route('a'));
        const [renamed, later] = await Promise.all([library.renameRoute('a', 'Lake loop'), library.saveRoute({ ...saved, updatedAt: 5 })]);
        expect(renamed).toMatchObject({ name: 'Lake loop' });
        expect(later).toMatchObject({ name: 'Lake loop', updatedAt: 5 });
        await expect(library.renameRoute('gone', 'x')).rejects.toThrow('not in the library');
        await library.removeRoute('a');
        expect(await library.saveRoute({ ...saved, updatedAt: 6 })).toMatchObject({ name: 'a', updatedAt: 6 });
        await library.close();
    });

    it('keeps a draft for each route, deletes it at a save, and writes only the summary of a stored route', async () => {
        const library = new Library(new IDBFactory()), heard: string[] = [];
        const draft = (id: string) => ({ id, updatedAt: 1, trip: { points: [id] } });
        await library.saveDraft(draft('new-1'));
        await library.saveDraft(draft('new-2'));
        expect([await library.draft('new-1'), await library.draft('new-2')]).toEqual([draft('new-1'), draft('new-2')]);
        await library.saveRoute(route('new-1'));
        expect([await library.draft('new-1'), await library.draft('new-2')]).toEqual([undefined, draft('new-2')]);
        await library.removeDraft('new-2');
        expect(await library.draft('new-2')).toBeUndefined();

        library.onChange((store, key) => heard.push(`${store}:${key}`));
        const summary = summarize<RouteSummary>([[8, 47, 400], [8.01, 47, 420]], { lengthM: 760, climbM: 20, seconds: 300, days: 1, bike: 'gravel' });
        // A summary made for another trip of the route is not written.
        await library.setSummary('new-1', summary, { points: ['other'] });
        expect((await library.route('new-1'))!.summary).toBeNull();
        await library.setSummary('new-1', summary, { points: [] });
        await library.setSummary('gone', summary, { points: [] });
        expect(await library.route('new-1')).toEqual({ ...route('new-1'), summary });
        expect([heard, await library.route('gone')]).toEqual([['routes:new-1'], undefined]);
        await library.close();
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
        await library.saveRoute(route('a'));
        await library.addRide(ride('20260801T070000Z'), new Blob(['ride']));
        await library.saveCover({ id: 'a', lineKey: 'k', state: 'none', tries: 0 });
        await library.setState('active', 'a');
        const before = library.saveRoute(route('b'));
        await library.clear();
        await expect(before).resolves.toMatchObject({ id: 'b' });
        await expect(library.saveRoute(route('c'))).rejects.toThrow('deleted');
        const fresh = new Library(factory), contents = await fresh.contents();
        expect([contents.routes, contents.rides, contents.files.size, contents.state, await fresh.cover('a')])
            .toEqual([[], [], 0, {}, undefined]);
        await library.close(); await fresh.close();
    });

    it('opens a database of version 1 with its routes and rides, adds the stores of the photos and the drafts, and deletes the versions and the fields `open` and `revision`', async () => {
        const factory = new IDBFactory(), key = '20260801T070000Z';
        await new Promise<void>((resolve, reject) => {
            const request = factory.open('obc-library', 1);
            request.onupgradeneeded = () => {
                const db = request.result;
                db.createObjectStore('routes', { keyPath: 'id' });
                db.createObjectStore('rides', { keyPath: 'key' });
                db.createObjectStore('covers', { keyPath: 'id' });
                for (const store of ['versions', 'ride-files', 'state']) db.createObjectStore(store);
            };
            request.onsuccess = () => {
                const db = request.result, tx = db.transaction(['routes', 'rides', 'ride-files'], 'readwrite');
                tx.objectStore('routes').put({ ...route('a'), open: 'v1', revision: 3 });
                tx.objectStore('rides').put(ride(key));
                tx.objectStore('ride-files').put(new Blob(['ride']), key);
                tx.oncomplete = () => { db.close(); resolve(); };
            };
            request.onerror = () => reject(request.error);
        });
        const library = new Library(factory);
        expect(await library.routes()).toEqual([route('a')]);
        expect((await library.rides()).map(r => r.key)).toEqual([key]);
        expect(await (await library.rideFile(key))!.text()).toBe('ride');
        expect(await library.addPhoto(photo('p1', key))).toBe(true);
        expect((await library.photos(key)).map(p => p.id)).toEqual(['p1']);
        await library.saveDraft({ id: 'a', updatedAt: 1, trip: {} });
        await library.close();
        const db = await new Promise<IDBDatabase>(resolve => { const request = factory.open('obc-library'); request.onsuccess = () => resolve(request.result); });
        expect(db.objectStoreNames.contains('versions')).toBe(false);
        db.close();
    });

    it('fails each call with one plain sentence after another tab upgrades the database', async () => {
        const factory = new IDBFactory(), library = new Library(factory);
        await library.saveRoute(route('a'));
        await new Promise<void>((resolve, reject) => {
            const request = factory.open('obc-library', 5);
            request.onsuccess = () => { request.result.close(); resolve(); };
            request.onerror = request.onblocked = () => reject(new Error('The upgrade did not run.'));
        });
        await expect(library.routes()).rejects.toThrow('The app was updated in another tab. Reload the page.');
        for (const write of [library.saveRoute(route('b')), library.saveDraft({ id: 'b', updatedAt: 1, trip: {} }), library.removeDraft('a'), library.renameRoute('a', 'b')])
            await expect(write).rejects.toMatchObject({ name: 'OutdatedError' });
    });

    it('adds a photo one time and only to a stored ride, and removes the photos of a ride with the ride', async () => {
        const library = new Library(new IDBFactory()), [first, second] = ['20260801T070000Z', '20260802T070000Z'];
        await library.addRide(ride(first), new Blob(['ride']));
        await library.addRide(ride(second), new Blob(['ride']));
        expect(await library.addPhoto(photo('p1', first))).toBe(true);
        expect(await library.addPhoto({ ...photo('p1', first), time: 5 })).toBe(false);
        expect(await library.addPhoto(photo('p2', '20260809T070000Z'))).toBe(false);
        await library.addPhoto(photo('p3', first));
        await library.addPhoto(photo('p4', second));
        await library.removePhoto('p3');
        expect((await library.photos(first)).map(p => [p.id, p.time])).toEqual([['p1', 1]]);
        expect(await (await library.photo('p1'))!.image.text()).toBe('copy of p1');
        await library.removeRide(first);
        expect(await library.photoIds()).toEqual(['p4']);
        await library.close();
    });
});
