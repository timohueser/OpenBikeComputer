import { IDBFactory } from 'fake-indexeddb';
import { describe, expect, it } from 'vitest';
import { emptyTrip, setEndpoint, pinNight, addRestDay, setDrawnLeg } from './editor';
import { PlanLibrary, exportPlan, importPlan, newPlan } from './library';
import { newVersion } from './versions';
import { testTrip } from '../../../test-support/planner/trip';

/** Runs `work` on the plans store of the library database, outside `PlanLibrary`. */
async function rawPlans(factory: IDBFactory, work: (plans: IDBObjectStore) => IDBRequest | void, version?: number): Promise<unknown> {
    const db = await new Promise<IDBDatabase>(resolve => {
        const request = factory.open('obc-planner-plans', version);
        request.onupgradeneeded = () => { request.result.createObjectStore('plans', { keyPath: 'id' }); request.result.createObjectStore('state'); };
        request.onsuccess = () => resolve(request.result);
    });
    const tx = db.transaction('plans', 'readwrite'), request = work(tx.objectStore('plans'));
    await new Promise(resolve => tx.oncomplete = resolve);
    db.close();
    return request?.result;
}

describe('local plan library', () => {
    it('keeps independent plans and their versions, resumes the active one and removes its pointer on deletion', async () => {
        const factory = new IDBFactory();
        const store = new PlanLibrary(factory);
        const first = await store.save(newPlan(testTrip(), 'Weekend'));
        const versioned = await store.save(first, [newVersion(first.trip, undefined, 'Valley')]);
        // An edit without versions leaves the saved versions as they are.
        await store.save({ ...versioned, name: 'Weekend edit' });
        const second = await store.save(newPlan(setEndpoint(emptyTrip(), 'start', [8, 48], 'Home'), 'Next ride'));
        await store.activate(second.id);
        await store.close();
        const reopened = new PlanLibrary(factory);
        expect((await reopened.active())?.id).toBe(second.id);
        expect((await reopened.list()).plans.map(p => p.name)).toEqual(['Next ride', 'Weekend edit']);
        expect((await reopened.versions(first.id)).map(v => v.name)).toEqual(['Valley']);
        expect(await reopened.versions(second.id)).toEqual([]);
        await reopened.remove((await reopened.get(first.id))!);
        expect(await reopened.versions(first.id)).toEqual([]);
        await reopened.remove(second);
        expect(await reopened.active()).toBeUndefined();
        expect((await reopened.list()).plans).toEqual([]);
        await reopened.close();
    });

    it('lists only valid plans, counts the others and deletes them on request', async () => {
        const factory = new IDBFactory();
        const store = new PlanLibrary(factory);
        const valid = await store.save(newPlan(testTrip(), 'Valid'));
        const broken = await store.save(newPlan(testTrip(), 'Broken'), [newVersion(testTrip(), undefined)]);
        await store.close();
        await rawPlans(factory, plans => { plans.put({ ...broken, trip: {} }); });
        const reopened = new PlanLibrary(factory);
        expect(await reopened.list()).toEqual({ plans: [valid], unreadable: 1 });
        await expect(reopened.get(broken.id)).rejects.toThrow('invalid');
        await reopened.removeUnreadable();
        expect(await reopened.list()).toEqual({ plans: [valid], unreadable: 0 });
        expect(await reopened.versions(broken.id)).toEqual([]);
        await reopened.close();
    });

    it('keeps the plans of a version 1 library and saves them without the versions they held', async () => {
        const factory = new IDBFactory();
        const old = { ...newPlan(testTrip(), 'Old'), revision: 1, versions: [newVersion(testTrip(), undefined, 'Inside')] };
        await rawPlans(factory, plans => { plans.put(old); }, 1);
        const store = new PlanLibrary(factory);
        const [listed] = (await store.list()).plans;
        expect(listed).toEqual({ id: old.id, revision: 1, name: 'Old', updatedAt: old.updatedAt, summary: old.summary, trip: old.trip });
        await store.save({ ...listed, name: 'Kept' });
        await store.close();
        expect(await rawPlans(factory, plans => plans.get(old.id))).not.toHaveProperty('versions');
    });

    it('orders rapid edits and refuses unseen edits or deletion from another tab', async () => {
        const factory = new IDBFactory();
        const first = new PlanLibrary(factory), second = new PlanLibrary(factory);
        const plan = await first.save(newPlan(testTrip(), 'Original'));
        await first.activate(plan.id);
        const stale = (await second.active())!;
        await Promise.all([first.save({ ...plan, name: 'First edit' }), first.save({ ...plan, name: 'Last edit' })]);
        expect((await first.list()).plans[0]).toMatchObject({ name: 'Last edit', revision: 3 });
        await expect(second.save({ ...stale, name: 'Unseen overwrite' })).rejects.toThrow('another tab');
        await expect(second.remove(stale)).rejects.toThrow('changed');
        const latest = (await second.get(plan.id))!;
        await second.save({ ...latest, name: 'After reopening' });
        expect((await first.list()).plans[0].name).toBe('After reopening');
        await first.close(); await second.close();
    });

    it('accepts a refreshed catalog revision while keeping stale active edits protected', async () => {
        const factory = new IDBFactory();
        const first = new PlanLibrary(factory), second = new PlanLibrary(factory);
        const original = await first.save(newPlan(testTrip(), 'Original'));
        const other = (await second.get(original.id))!;
        await second.save({ ...other, name: 'Other tab' });
        const [refreshed] = (await first.list()).plans;
        await expect(first.save({ ...original, name: 'Stale active edit' })).rejects.toThrow('another tab');
        const renamed = await first.save({ ...refreshed, name: 'Catalog rename' });
        expect(renamed).toMatchObject({ name: 'Catalog rename', revision: 3 });
        await expect(second.save({ ...other, name: 'Unseen edit' })).rejects.toThrow('another tab');
        await first.close(); await second.close();
    });

    it('round-trips nights, dates, rest days and drawings with its versions under a new identity', () => {
        const trip = { ...addRestDay(pinNight(testTrip(), undefined, 1, [7.2, 47.5], 'Camp'), 1), startDate: '2026-10-04' };
        const drawn = setDrawnLeg(trip, 'finish', [[7, 47.5]]);
        const plan = newPlan(drawn, 'Autumn tour'), versions = [newVersion(trip, undefined, 'Before the detour')];
        const imported = importPlan(exportPlan(plan, versions));
        expect(imported.plan.id).not.toBe(plan.id);
        expect(imported.plan).toMatchObject({ revision: 0, name: plan.name, trip: JSON.parse(JSON.stringify(drawn)) });
        expect(imported.versions).toEqual(JSON.parse(JSON.stringify(versions)));
    });

    it('rejects malformed plans and versions without accepting part of a file', () => {
        const file = JSON.parse(exportPlan(newPlan(testTrip(), 'Valid'), [newVersion(testTrip(), undefined)]));
        for (const invalid of [null, {}, { ...file, version: 99 }, { ...file, trip: {} },
            { ...file, trip: { ...file.trip, startDate: '2026-02-30' } },
            { ...file, trip: { ...file.trip, points: [{ ...file.trip.points[0], anchor: [999, 0] }, file.trip.points[1]] } },
            { ...file, versions: [null] }, { ...file, versions: [file.versions[0], file.versions[0]] },
            { ...file, versions: [{ ...file.versions[0], trip: {} }] }]) {
            expect(() => importPlan(JSON.stringify(invalid))).toThrow();
        }
        expect(() => importPlan('{')).toThrow();
    });
});
