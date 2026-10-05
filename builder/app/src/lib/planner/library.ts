import type { Trip } from './editor';
import { isTrip } from './trip-validation';
import { planTitle, versionSummary, type Version } from './versions';

export interface Plan {
    id: string;
    revision: number;
    name: string;
    updatedAt: number;
    summary: string;
    trip: Trip;
    versions: Version[];
}

export function newPlan(trip: Trip, name = '', versions: Version[] = []): Plan {
    return { id: crypto.randomUUID(), revision: 0, name: name.trim().slice(0, 120), updatedAt: Date.now(), summary: versionSummary(trip, undefined), trip, versions };
}

/** Writes are ordered within a tab; revisions prevent a second tab from replacing unseen edits. */
export class PlanLibrary {
    private database?: Promise<IDBDatabase>;
    private pending: Promise<unknown> = Promise.resolve();
    private revisions = new Map<string, number>();

    constructor(private factory: IDBFactory = indexedDB) {}

    private open(): Promise<IDBDatabase> {
        return this.database ??= new Promise<IDBDatabase>((resolve, reject) => {
            const request = this.factory.open('obc-planner-plans', 1);
            request.onupgradeneeded = () => {
                request.result.createObjectStore('plans', { keyPath: 'id' });
                request.result.createObjectStore('state');
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

    async list(): Promise<Plan[]> {
        await this.pending;
        const plans = await this.read('plans') as Plan[];
        return plans.sort((a, b) => b.updatedAt - a.updatedAt);
    }

    async active(): Promise<Plan | undefined> {
        const id = await this.read('state', 'active');
        if (typeof id !== 'string') return undefined;
        return this.get(id);
    }

    async get(id: string): Promise<Plan | undefined> {
        const plan = await this.read('plans', id) as Plan | undefined;
        if (plan && !isTrip(plan.trip)) throw new Error('Saved plan is invalid. Open another plan or import a backup.');
        if (plan) this.revisions.set(id, plan.revision);
        return plan;
    }

    private enqueue<T>(work: () => Promise<T>): Promise<T> {
        const result = this.pending.then(work);
        this.pending = result.catch(() => {});
        return result;
    }

    save(plan: Plan): Promise<Plan> {
        const snapshot = structuredClone(plan);
        return this.enqueue(async () => {
            const db = await this.open();
            return new Promise<Plan>((resolve, reject) => {
                const tx = db.transaction('plans', 'readwrite');
                const store = tx.objectStore('plans');
                const request = store.get(snapshot.id);
                let next: Plan;
                let conflict = false;
                let failure: unknown;
                request.onsuccess = () => {
                    const expected = Math.max(this.revisions.get(snapshot.id) ?? 0, snapshot.revision);
                    if ((request.result?.revision ?? 0) !== expected) { conflict = true; tx.abort(); return; }
                    next = { ...snapshot, revision: expected + 1 };
                    try { store.put(next); }
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
                const tx = db.transaction(['plans', 'state'], 'readwrite');
                const store = tx.objectStore('plans');
                const request = store.get(plan.id);
                let conflict = false;
                request.onsuccess = () => {
                    if (request.result && request.result.revision !== plan.revision) { conflict = true; tx.abort(); return; }
                    store.delete(plan.id);
                };
                const active = tx.objectStore('state').get('active');
                active.onsuccess = () => { if (active.result === plan.id) tx.objectStore('state').delete('active'); };
                tx.oncomplete = () => resolve();
                tx.onabort = () => reject(conflict ? new Error('This plan changed. Reopen My plans before deleting it.') : tx.error);
            });
        });
    }

    async close(): Promise<void> { (await this.database)?.close(); }
}

export function exportPlan(plan: Plan): string {
    return JSON.stringify({ format: 'openbikecomputer-plan', version: 1, name: (plan.name || planTitle(plan.trip)).slice(0, 120),
        trip: plan.trip, versions: plan.versions }, null, 2);
}

export function importPlan(text: string): Plan {
    const file = JSON.parse(text);
    if (!file || file.format !== 'openbikecomputer-plan' || file.version !== 1 || typeof file.name !== 'string'
        || !file.name.trim() || file.name.length > 120 || !Array.isArray(file.versions)) throw new Error('Not an OpenBikeComputer plan file.');
    if (!isTrip(file.trip)) throw new Error('The file contains an invalid plan.');
    const ids = new Set<string>();
    const versions = file.versions.map((value: unknown): Version => {
        const v = value as Partial<Version> | null;
        if (!v || !isTrip(v.trip) || typeof v.id !== 'string' || !v.id || ids.has(v.id) || typeof v.at !== 'string'
            || !Number.isFinite(Date.parse(v.at)) || new Date(v.at).toISOString() !== v.at || (v.name !== undefined && (typeof v.name !== 'string' || v.name.length > 120)))
            throw new Error('The file contains an invalid version.');
        ids.add(v.id);
        return { id: v.id, at: v.at, ...(v.name ? { name: v.name } : {}), summary: versionSummary(v.trip, undefined), trip: v.trip };
    });
    return newPlan(file.trip, file.name.trim(), versions);
}

export function downloadPlan(plan: Plan): void {
    const url = URL.createObjectURL(new Blob([exportPlan(plan)], { type: 'application/json' }));
    const link = document.createElement('a');
    link.href = url;
    link.download = `${(plan.name || planTitle(plan.trip)).replace(/[^\p{L}\p{N} _-]/gu, '').slice(0, 80) || 'plan'}.obcplan`;
    link.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
}
