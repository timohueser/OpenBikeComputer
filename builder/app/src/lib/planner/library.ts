import type { Trip } from './editor';
import { isTrip } from './trip-validation';
import { planTitle, versionSummary, type Version } from './versions';

/** A plan record of the browser library. Its versions are a separate record, written only when they change. */
export interface Plan {
    id: string;
    revision: number;
    name: string;
    updatedAt: number;
    summary: string;
    trip: Trip;
}

export function newPlan(trip: Trip, name = ''): Plan {
    return { id: crypto.randomUUID(), revision: 0, name: name.trim().slice(0, 120), updatedAt: Date.now(), summary: versionSummary(trip, undefined), trip };
}

function isPlan(value: unknown): value is Plan {
    const plan = value as Partial<Plan> | null;
    return !!plan && typeof plan.id === 'string' && !!plan.id && Number.isInteger(plan.revision) && typeof plan.name === 'string'
        && Number.isFinite(plan.updatedAt) && typeof plan.summary === 'string' && isTrip(plan.trip);
}

// Only the fields of a plan: a record of a version 1 library also holds its versions, which a save must not write back.
function fields({ id, revision, name, updatedAt, summary, trip }: Plan): Plan {
    return { id, revision, name, updatedAt, summary, trip };
}

function isVersion(value: unknown): value is Version {
    const v = value as Partial<Version> | null;
    return !!v && isTrip(v.trip) && typeof v.id === 'string' && !!v.id && typeof v.at === 'string' && Number.isFinite(Date.parse(v.at))
        && new Date(v.at).toISOString() === v.at && (v.name === undefined || (typeof v.name === 'string' && v.name.length <= 120));
}

/** Writes are ordered within a tab; revisions prevent a second tab from replacing unseen edits. */
export class PlanLibrary {
    private database?: Promise<IDBDatabase>;
    private pending: Promise<unknown> = Promise.resolve();
    private revisions = new Map<string, number>();

    constructor(private factory: IDBFactory = indexedDB) {}

    private open(): Promise<IDBDatabase> {
        return this.database ??= new Promise<IDBDatabase>((resolve, reject) => {
            const request = this.factory.open('obc-planner-plans', 2);
            request.onupgradeneeded = () => {
                const db = request.result;
                // A version 1 library keeps its plans; the versions inside their records are lost.
                if (!db.objectStoreNames.contains('plans')) db.createObjectStore('plans', { keyPath: 'id' });
                for (const store of ['versions', 'state']) if (!db.objectStoreNames.contains(store)) db.createObjectStore(store);
            };
            request.onsuccess = () => {
                const db = request.result;
                db.onversionchange = () => db.close();
                resolve(db);
            };
            request.onerror = () => reject(request.error);
            request.onblocked = () => reject(new Error('Close other planner tabs and try again.'));
        }).catch(error => { this.database = undefined; throw error; });
    }

    private async read(store: string, key?: string): Promise<unknown> {
        const db = await this.open();
        return new Promise((resolve, reject) => {
            const tx = db.transaction(store);
            const request = key === undefined ? tx.objectStore(store).getAll() : tx.objectStore(store).get(key);
            tx.oncomplete = () => resolve(request.result);
            tx.onabort = () => reject(tx.error);
        });
    }

    /** The valid plans, most recently edited first, and the number of records that are not valid plans. */
    async list(): Promise<{ plans: Plan[]; unreadable: number }> {
        await this.pending;
        const records = await this.read('plans') as unknown[];
        const plans = records.filter(isPlan).map(fields).sort((a, b) => b.updatedAt - a.updatedAt);
        return { plans, unreadable: records.length - plans.length };
    }

    /** The valid versions of a plan, newest first. */
    async versions(id: string): Promise<Version[]> {
        const versions = await this.read('versions', id);
        return Array.isArray(versions) ? versions.filter(v => isVersion(v) && typeof v.summary === 'string') : [];
    }

    async active(): Promise<Plan | undefined> {
        const id = await this.read('state', 'active');
        if (typeof id !== 'string') return undefined;
        return this.get(id);
    }

    async get(id: string): Promise<Plan | undefined> {
        const plan = await this.read('plans', id);
        if (plan === undefined) return undefined;
        if (!isPlan(plan)) throw new Error('Saved plan is invalid. Open another plan or import a backup.');
        this.revisions.set(id, plan.revision);
        return fields(plan);
    }

    private enqueue<T>(work: () => Promise<T>): Promise<T> {
        const result = this.pending.then(work);
        this.pending = result.catch(() => {});
        return result;
    }

    /** Saves the plan, and its versions when they are given. */
    save(plan: Plan, versions?: Version[]): Promise<Plan> {
        const snapshot = structuredClone(plan), kept = versions && structuredClone(versions);
        return this.enqueue(async () => {
            const db = await this.open();
            return new Promise<Plan>((resolve, reject) => {
                const tx = db.transaction(kept ? ['plans', 'versions'] : 'plans', 'readwrite');
                const store = tx.objectStore('plans');
                const request = store.get(snapshot.id);
                let next: Plan;
                let conflict = false;
                let failure: unknown;
                request.onsuccess = () => {
                    const expected = Math.max(this.revisions.get(snapshot.id) ?? 0, snapshot.revision);
                    if ((request.result?.revision ?? 0) !== expected) { conflict = true; tx.abort(); return; }
                    next = { ...snapshot, revision: expected + 1 };
                    try {
                        store.put(next);
                        if (kept) tx.objectStore('versions').put(kept, next.id);
                    }
                    catch (error) { failure = error; tx.abort(); }
                };
                tx.oncomplete = () => { this.revisions.set(next.id, next.revision); resolve(next); };
                tx.onabort = () => reject(conflict ? new Error('This plan changed in another tab. Download your edits before reopening it.') : failure ?? tx.error);
            });
        });
    }

    activate(id: string | null): Promise<void> {
        return this.enqueue(async () => {
            const db = await this.open();
            await new Promise<void>((resolve, reject) => {
                const tx = db.transaction('state', 'readwrite');
                tx.objectStore('state').put(id, 'active');
                tx.oncomplete = () => resolve();
                tx.onabort = () => reject(tx.error);
            });
        });
    }

    remove(plan: Plan): Promise<void> {
        return this.enqueue(async () => {
            const db = await this.open();
            await new Promise<void>((resolve, reject) => {
                const tx = db.transaction(['plans', 'versions', 'state'], 'readwrite');
                const store = tx.objectStore('plans');
                const request = store.get(plan.id);
                let conflict = false;
                request.onsuccess = () => {
                    if (request.result && request.result.revision !== plan.revision) { conflict = true; tx.abort(); return; }
                    store.delete(plan.id);
                    tx.objectStore('versions').delete(plan.id);
                };
                const active = tx.objectStore('state').get('active');
                active.onsuccess = () => { if (active.result === plan.id) tx.objectStore('state').delete('active'); };
                tx.oncomplete = () => resolve();
                tx.onabort = () => reject(conflict ? new Error('This plan changed. Reopen My plans before deleting it.') : tx.error);
            });
        });
    }

    /** Deletes the records that are not valid plans, with their versions. */
    removeUnreadable(): Promise<void> {
        return this.enqueue(async () => {
            const db = await this.open();
            await new Promise<void>((resolve, reject) => {
                const tx = db.transaction(['plans', 'versions'], 'readwrite');
                const cursor = tx.objectStore('plans').openCursor();
                cursor.onsuccess = () => {
                    const record = cursor.result;
                    if (!record) return;
                    if (!isPlan(record.value)) { record.delete(); tx.objectStore('versions').delete(record.primaryKey); }
                    record.continue();
                };
                tx.oncomplete = () => resolve();
                tx.onabort = () => reject(tx.error);
            });
        });
    }

    async close(): Promise<void> { (await this.database)?.close(); }
}

export function exportPlan(plan: Plan, versions: Version[]): string {
    return JSON.stringify({ format: 'openbikecomputer-plan', version: 1, name: (plan.name || planTitle(plan.trip)).slice(0, 120),
        trip: plan.trip, versions }, null, 2);
}

export function importPlan(text: string): { plan: Plan; versions: Version[] } {
    const file = JSON.parse(text);
    if (!file || file.format !== 'openbikecomputer-plan' || file.version !== 1 || typeof file.name !== 'string'
        || !file.name.trim() || file.name.length > 120 || !Array.isArray(file.versions)) throw new Error('Not an OpenBikeComputer plan file.');
    if (!isTrip(file.trip)) throw new Error('The file contains an invalid plan.');
    const ids = new Set<string>();
    const versions = file.versions.map((value: unknown): Version => {
        if (!isVersion(value) || ids.has(value.id)) throw new Error('The file contains an invalid version.');
        ids.add(value.id);
        return { id: value.id, at: value.at, ...(value.name ? { name: value.name } : {}), summary: versionSummary(value.trip, undefined), trip: value.trip };
    });
    return { plan: newPlan(file.trip, file.name.trim()), versions };
}

export function downloadPlan(plan: Plan, versions: Version[]): void {
    const url = URL.createObjectURL(new Blob([exportPlan(plan, versions)], { type: 'application/json' }));
    const link = document.createElement('a');
    link.href = url;
    link.download = `${(plan.name || planTitle(plan.trip)).replace(/[^\p{L}\p{N} _-]/gu, '').slice(0, 80) || 'plan'}.obcplan`;
    link.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
}
