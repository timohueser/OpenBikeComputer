/** Keeps the routes, the rides, their source files, the covers and the library state in the browser database `obc-library`. */
import { isRideRecord, isRouteRecord, stateChecks, type CoverRecord, type LibraryState, type RideRecord, type RouteRecord } from './records';

export type StoreName = 'routes' | 'versions' | 'rides' | 'ride-files' | 'covers' | 'state';

/** All records of the library except the covers: the content of a backup. */
export interface Contents {
    routes: RouteRecord[];
    /** The versions of each route, by route id. */
    versions: Record<string, unknown[]>;
    rides: RideRecord[];
    /** The source file of each ride, by ride key. */
    files: Map<string, Blob>;
    state: Partial<LibraryState>;
}

type Fail = (error: unknown) => void;

const conflict = () => new Error('This route changed in another tab. Download your edits before reopening it.');

/** Writes are ordered within a tab; revisions prevent a second tab from replacing unseen edits of a route. */
export class Library {
    private database?: Promise<IDBDatabase>;
    private pending: Promise<unknown> = Promise.resolve();
    private revisions = new Map<string, number>();
    private listeners = new Set<(store: StoreName, key: string) => void>();
    private written = false;

    /** Without `factory`, `open` reads `indexedDB`, so a browser that blocks storage gives a rejected promise. */
    constructor(private factory?: IDBFactory) {}

    private open(): Promise<IDBDatabase> {
        return this.database ??= new Promise<IDBDatabase>((resolve, reject) => {
            const request = (this.factory ?? indexedDB).open('obc-library', 1);
            request.onupgradeneeded = () => {
                const db = request.result;
                db.createObjectStore('routes', { keyPath: 'id' });
                db.createObjectStore('rides', { keyPath: 'key' });
                db.createObjectStore('covers', { keyPath: 'id' });
                for (const store of ['versions', 'ride-files', 'state']) db.createObjectStore(store);
            };
            request.onsuccess = () => {
                const db = request.result;
                db.onversionchange = () => db.close();
                resolve(db);
            };
            request.onerror = () => reject(request.error);
            request.onblocked = () => reject(new Error('Close other tabs of the app and try again.'));
        }).catch(error => { this.database = undefined; throw error; });
    }

    /** Runs `work` in one transaction and resolves with the value of the function it returns, after the commit. */
    private async run<T>(stores: StoreName[], mode: IDBTransactionMode, work: (tx: IDBTransaction, fail: Fail) => () => T): Promise<T> {
        const db = await this.open();
        return new Promise<T>((resolve, reject) => {
            const tx = db.transaction(stores, mode);
            let failure: unknown, result: () => T = () => undefined as T;
            const fail: Fail = error => { failure = error; tx.abort(); };
            try { result = work(tx, fail); } catch (error) { fail(error); }
            tx.oncomplete = () => resolve(result());
            tx.onabort = () => reject(failure ?? tx.error);
        });
    }

    private read(store: StoreName, key?: string): Promise<unknown> {
        return this.run([store], 'readonly', tx => {
            const request = key === undefined ? tx.objectStore(store).getAll() : tx.objectStore(store).get(key);
            return () => request.result;
        });
    }

    /** A write in the order of this tab. `changed` names each record that it writes, for the listeners of `onChange`. */
    private write<T>(stores: StoreName[], work: (tx: IDBTransaction, fail: Fail, changed: (store: StoreName, key: string) => void) => () => T): Promise<T> {
        const result = this.pending.then(async () => {
            const changes: [StoreName, string][] = [];
            const value = await this.run(stores, 'readwrite', (tx, fail) => work(tx, fail, (store, key) => changes.push([store, key])));
            for (const [store, key] of changes) for (const listener of this.listeners) listener(store, key);
            if (!this.written) { this.written = true; void this.persist(); }
            return value;
        });
        this.pending = result.catch(() => {});
        return result;
    }

    /** Asks the browser to keep the library when the disk is full. Firefox asks the rider. */
    async persist(): Promise<boolean> {
        try { return await navigator.storage?.persist?.() ?? false; } catch { return false; }
    }

    /** Calls `listener` after each record that this tab writes or removes. Returns the function that stops it. */
    onChange(listener: (store: StoreName, key: string) => void): () => void {
        this.listeners.add(listener);
        return () => this.listeners.delete(listener);
    }

    async routes(): Promise<RouteRecord[]> {
        await this.pending;
        return (await this.read('routes') as unknown[]).filter(isRouteRecord);
    }

    /** The ids of the records of `routes` that are not route records. `routes()` and a backup leave them out. */
    async unreadableRoutes(): Promise<string[]> {
        await this.pending;
        return this.run(['routes'], 'readonly', tx => {
            const keys = tx.objectStore('routes').getAllKeys(), values = tx.objectStore('routes').getAll();
            return () => keys.result.filter((_, i) => !isRouteRecord(values.result[i])) as string[];
        });
    }

    async route(id: string): Promise<RouteRecord | undefined> {
        const route = await this.read('routes', id);
        if (route === undefined) return undefined;
        if (!isRouteRecord(route)) throw new Error('This saved route cannot be read.');
        this.revisions.set(id, route.revision);
        return route;
    }

    /** The saved versions of a route, newest first. The planner checks them. */
    async versions(id: string): Promise<unknown[]> {
        const versions = await this.read('versions', id);
        return Array.isArray(versions) ? versions : [];
    }

    /** Saves the route with the next revision, and its versions when they are given. It refuses a route that another tab
     * saved after this tab read it. */
    saveRoute<R extends RouteRecord>(route: R, versions?: unknown[]): Promise<R> {
        const snapshot = structuredClone(route), kept = versions && structuredClone(versions);
        return this.write(kept ? ['routes', 'versions'] : ['routes'], (tx, fail, changed) => {
            const store = tx.objectStore('routes'), request = store.get(snapshot.id);
            const expected = Math.max(this.revisions.get(snapshot.id) ?? 0, snapshot.revision);
            const next = { ...snapshot, revision: expected + 1 };
            request.onsuccess = () => {
                if ((request.result?.revision ?? 0) !== expected) return fail(conflict());
                try {
                    store.put(next);
                    if (kept) tx.objectStore('versions').put(kept, next.id);
                    changed('routes', next.id);
                } catch (error) { fail(error); }
            };
            return () => { this.revisions.set(next.id, next.revision); return next; };
        });
    }

    /** Removes a route with its versions and its cover. With `revision`, it refuses a route that changed since then. */
    removeRoute(id: string, revision?: number): Promise<void> {
        return this.write(['routes', 'versions', 'covers', 'state'], (tx, fail, changed) => {
            const store = tx.objectStore('routes'), request = store.get(id), state = tx.objectStore('state');
            request.onsuccess = () => {
                if (revision !== undefined && request.result && request.result.revision !== revision) return fail(conflict());
                store.delete(id);
                tx.objectStore('versions').delete(id);
                tx.objectStore('covers').delete(id);
                changed('routes', id);
            };
            const active = state.get('active');
            active.onsuccess = () => { if (active.result === id) state.put(null, 'active'); };
            return () => {};
        });
    }

    /** A route or a ride by its id or key. It does not record the revision as `route` does, so a reader such as the cover
     * job does not hide the edits of another tab from the planner. */
    async item(id: string): Promise<RouteRecord | RideRecord | undefined> {
        const route = await this.read('routes', id);
        return isRouteRecord(route) ? route : this.ride(id);
    }

    async rides(): Promise<RideRecord[]> {
        await this.pending;
        return (await this.read('rides') as unknown[]).filter(isRideRecord);
    }

    async ride(key: string): Promise<RideRecord | undefined> {
        const ride = await this.read('rides', key);
        return isRideRecord(ride) ? ride : undefined;
    }

    /** The source file of a ride, unchanged. */
    async rideFile(key: string): Promise<Blob | undefined> {
        const file = await this.read('ride-files', key);
        return file instanceof Blob ? file : undefined;
    }

    /** Replaces the record of a stored ride, for example after a new name. */
    saveRide(ride: RideRecord): Promise<void> {
        const record = structuredClone(ride);
        return this.write(['rides'], (tx, _, changed) => {
            tx.objectStore('rides').put(record);
            changed('rides', record.key);
            return () => {};
        });
    }

    /** Adds a ride and its source file. False when the key is stored or deleted: then it only writes the device id of
     * `ride`, so `deviceRides` skips it. One transaction checks and writes, so two tabs add a ride once. */
    addRide(ride: RideRecord, file: Blob): Promise<boolean> {
        const record = structuredClone(ride);
        return this.write(['rides', 'ride-files', 'state'], (tx, _, changed) => {
            const rides = tx.objectStore('rides'), state = tx.objectStore('state');
            const stored = rides.get(record.key), list = state.get('deleted');
            let added = false;
            list.onsuccess = () => {
                const deleted = stateChecks.deleted(list.result) ? list.result : [];
                const gone = deleted.find(d => d.key === record.key), device = record.device;
                if (stored.result) {
                    if (device && stored.result.device !== device) rides.put({ ...stored.result, device });
                } else if (gone) {
                    if (device && gone.device !== device) state.put(deleted.map(d => d === gone ? { ...d, device } : d), 'deleted');
                } else {
                    rides.put(record);
                    tx.objectStore('ride-files').put(file, record.key);
                    changed('rides', record.key);
                    added = true;
                }
            };
            return () => added;
        });
    }

    /** Removes a ride, its file and its cover, and adds its key to the deleted rides. */
    removeRide(key: string): Promise<void> {
        return this.write(['rides', 'ride-files', 'covers', 'state'], (tx, _, changed) => {
            const state = tx.objectStore('state'), ride = tx.objectStore('rides').get(key), list = state.get('deleted');
            list.onsuccess = () => {
                const deleted = stateChecks.deleted(list.result) ? list.result : [];
                const device: string | undefined = ride.result?.device;
                if (!deleted.some(d => d.key === key)) state.put([...deleted, { key, ...device && { device } }], 'deleted');
                tx.objectStore('rides').delete(key);
                tx.objectStore('ride-files').delete(key);
                tx.objectStore('covers').delete(key);
                changed('rides', key);
            };
            return () => {};
        });
    }

    /** The device ids of the stored and the deleted rides: the rides on the device to skip. */
    async deviceRides(): Promise<Set<string>> {
        const [rides, deleted] = await Promise.all([this.rides(), this.state('deleted')]);
        return new Set([...rides, ...deleted ?? []].flatMap(ride => ride.device ? [ride.device] : []));
    }

    async cover(id: string): Promise<CoverRecord | undefined> {
        return await this.read('covers', id) as CoverRecord | undefined;
    }

    /** Saves a cover only while its route or ride exists: a cover that renders while the rider deletes its record is dropped. */
    saveCover(cover: CoverRecord): Promise<void> {
        return this.write(['covers', 'routes', 'rides'], (tx, _, changed) => {
            const route = tx.objectStore('routes').getKey(cover.id), ride = tx.objectStore('rides').getKey(cover.id);
            ride.onsuccess = () => {
                if (route.result === undefined && ride.result === undefined) return;
                tx.objectStore('covers').put(cover);
                changed('covers', cover.id);
            };
            return () => {};
        });
    }

    async state<K extends keyof LibraryState>(key: K): Promise<LibraryState[K] | undefined> {
        const value = await this.read('state', key);
        return stateChecks[key](value) ? value as LibraryState[K] : undefined;
    }

    setState<K extends keyof LibraryState>(key: K, value: LibraryState[K]): Promise<void> {
        return this.write(['state'], (tx, _, changed) => {
            tx.objectStore('state').put(value, key);
            changed('state', key);
            return () => {};
        });
    }

    /** All records except the covers, read in one transaction. */
    contents(): Promise<Contents> {
        return this.run(['routes', 'versions', 'rides', 'ride-files', 'state'], 'readonly', tx => {
            const all = (store: StoreName) => ({ keys: tx.objectStore(store).getAllKeys(), values: tx.objectStore(store).getAll() });
            const routes = tx.objectStore('routes').getAll(), rides = tx.objectStore('rides').getAll();
            const versions = all('versions'), files = all('ride-files'), state = all('state');
            const pairs = ({ keys, values }: ReturnType<typeof all>) => keys.result.map((key, i) => [key as string, values.result[i]] as const);
            return () => ({
                routes: routes.result.filter(isRouteRecord),
                versions: Object.fromEntries(pairs(versions).filter(([, value]) => Array.isArray(value))) as Record<string, unknown[]>,
                rides: rides.result.filter(isRideRecord),
                files: new Map(pairs(files).filter((pair): pair is readonly [string, Blob] => pair[1] instanceof Blob)),
                state: Object.fromEntries(pairs(state).filter(([key, value]) => Object.hasOwn(stateChecks, key) && stateChecks[key as keyof LibraryState](value))),
            });
        });
    }

    /** Adds in one transaction each route whose id is new, and each ride whose key is neither stored nor deleted. It takes
     * the state of `contents` only when the library is empty: no route, no ride and no deleted ride. */
    merge(contents: Contents): Promise<{ routes: number; rides: number; empty: boolean }> {
        return this.write(['routes', 'versions', 'rides', 'ride-files', 'state'], (tx, _, changed) => {
            const routes = tx.objectStore('routes'), rides = tx.objectStore('rides'), state = tx.objectStore('state');
            const routeIds = routes.getAllKeys(), rideKeys = rides.getAllKeys(), list = state.get('deleted');
            const result = { routes: 0, rides: 0, empty: false };
            list.onsuccess = () => {
                const local = stateChecks.deleted(list.result) ? list.result : [];
                result.empty = !routeIds.result.length && !rideKeys.result.length && !local.length;
                if (result.empty) for (const [key, value] of Object.entries(contents.state)) state.put(value, key);
                const deleted = result.empty ? contents.state.deleted ?? [] : local;
                const skip = new Set([...rideKeys.result, ...deleted.map(d => d.key)]), known = new Set(routeIds.result);
                for (const route of contents.routes) {
                    if (known.has(route.id)) continue;
                    routes.put(route);
                    if (contents.versions[route.id]) tx.objectStore('versions').put(contents.versions[route.id], route.id);
                    changed('routes', route.id);
                    result.routes++;
                }
                for (const ride of contents.rides) {
                    if (skip.has(ride.key)) continue;
                    rides.put(ride);
                    tx.objectStore('ride-files').put(contents.files.get(ride.key), ride.key);
                    changed('rides', ride.key);
                    result.rides++;
                }
            };
            return () => result;
        });
    }

    async close(): Promise<void> { (await this.database)?.close(); }
}
