import { IDBFactory } from 'fake-indexeddb';
import { describe, expect, it } from 'vitest';
import { initialTrip, emptyTrip, setEndpoint, pinNight, addRestDay, setDrawnLeg, routingKey, orderedRoutePoints, closeLoop } from './editor';
import { PlanLibrary, exportPlan, importPlan, newPlan } from './library';
import { newVersion } from './versions';

describe('local plan library', () => {
    it('keeps independent plans and checkpoints, resumes the active one and removes its pointer on deletion', async () => {
        const factory = new IDBFactory();
        const store = new PlanLibrary(factory);
        const first = await store.save(newPlan(initialTrip(), 'Weekend'));
        const next = { ...first, versions: [newVersion(first.trip, 'Valley')] };
        await store.save(next);
        const second = await store.save(newPlan(setEndpoint(emptyTrip(), 'start', [8, 48], 'Home'), 'Next ride'));
        await store.activate(second.id);
        await store.close();
        const reopened = new PlanLibrary(factory);
        expect((await reopened.active())?.id).toBe(second.id);
        const plans = await reopened.list();
        expect(plans).toHaveLength(2);
        expect(plans.find(p => p.id === first.id)?.versions[0].name).toBe('Valley');
        expect(plans.find(p => p.id === second.id)?.versions).toEqual([]);
        await reopened.remove(second);
        expect(await reopened.active()).toBeUndefined();
        expect(await reopened.list()).toHaveLength(1);
        await reopened.close();
    });

    it('orders rapid edits and refuses unseen edits or deletion from another tab', async () => {
        const factory = new IDBFactory();
        const first = new PlanLibrary(factory), second = new PlanLibrary(factory);
        const plan = await first.save(newPlan(initialTrip(), 'Original'));
        await first.activate(plan.id);
        const stale = (await second.active())!;
        await Promise.all([first.save({ ...plan, name: 'First edit' }), first.save({ ...plan, name: 'Last edit' })]);
        expect((await first.list())[0]).toMatchObject({ name: 'Last edit', revision: 3 });
        await expect(second.save({ ...stale, name: 'Unseen overwrite' })).rejects.toThrow('another tab');
        await expect(second.remove(stale)).rejects.toThrow('changed');
        const latest = (await second.get(plan.id))!;
        await second.save({ ...latest, name: 'After reopening' });
        expect((await first.list())[0].name).toBe('After reopening');
        await first.close(); await second.close();
    });

    it('accepts a refreshed catalog revision while keeping stale active edits protected', async () => {
        const factory = new IDBFactory();
        const first = new PlanLibrary(factory), second = new PlanLibrary(factory);
        const original = await first.save(newPlan(initialTrip(), 'Original'));
        const other = (await second.get(original.id))!;
        await second.save({ ...other, name: 'Other tab' });
        const [refreshed] = await first.list();
        await expect(first.save({ ...original, name: 'Stale active edit' })).rejects.toThrow('another tab');
        const renamed = await first.save({ ...refreshed, name: 'Catalog rename' });
        expect(renamed).toMatchObject({ name: 'Catalog rename', revision: 3 });
        await expect(second.save({ ...other, name: 'Unseen edit' })).rejects.toThrow('another tab');
        await first.close(); await second.close();
    });

    it('round-trips nights, dates, rest days, drawings and picked geometry with its versions under a new identity', () => {
        const trip = { ...addRestDay(pinNight(initialTrip(), 1, [7.2, 47.5], 'Camp'), 1), startDate: '2026-10-04' };
        const drawn = setDrawnLeg(trip, 'finish', [[7, 47.5]]);
        const points = orderedRoutePoints(drawn);
        const coordinates = points.map(p => p.coordinate);
        const picked = { ...drawn, routing: { key: routingKey(drawn), choiceId: 'corridor', profile: 'touring', picked: true,
            coordinates, elevation: coordinates.map(() => 10), elapsed: coordinates.map((_, i) => i * 10), edges: {},
            stops: points.map((p, i) => ({ id: p.id, distance: i })), seconds: 20,
            unknownSurfaceKm: 0, pushingKm: 0, unroutedKm: 0, alternatives: [], alternativesReady: false } };
        const plan = newPlan(picked, 'Autumn tour', [newVersion(drawn, 'Before the detour')]);
        const imported = importPlan(exportPlan(plan));
        expect(imported.id).not.toBe(plan.id);
        expect(imported).toMatchObject({ revision: 0, name: plan.name, trip: JSON.parse(JSON.stringify(picked)), versions: JSON.parse(JSON.stringify(plan.versions)) });
    });

    it('requires ordered stops with increasing progress and keeps the repeated loop start', () => {
        const loop = { ...closeLoop(initialTrip()), live: true };
        const points = orderedRoutePoints(loop);
        const coordinates = points.map(p => p.coordinate);
        const stops = points.map((p, i) => ({ id: p.id, distance: i }));
        const routing = { key: routingKey(loop), choiceId: 'loop', profile: 'touring', picked: true,
            coordinates, elevation: points.map(() => null), elapsed: points.map((_, i) => i * 10), edges: {},
            stops, seconds: 20, unknownSurfaceKm: 0, pushingKm: 0, unroutedKm: 0,
            alternatives: [], alternativesReady: false };
        const file = JSON.parse(exportPlan(newPlan({ ...loop, routing }, 'Loop')));
        expect(importPlan(JSON.stringify(file)).trip.routing?.stops).toEqual(stops);
        expect(stops[0].id).toBe(stops.at(-1)!.id);
        for (const invalid of [[], stops.slice(0, -1), [...stops, stops[0]],
            [stops[1], stops[0], stops[2]], stops.map((p, i) => ({ ...p, distance: i ? 1 / i : 0 })),
            stops.map(p => ({ ...p, distance: 0 })), stops.map(p => ({ ...p, distance: p.distance + 1 }))]) {
            expect(() => importPlan(JSON.stringify({ ...file, trip: { ...file.trip, routing: { ...routing, stops: invalid } } }))).toThrow();
        }
        const equalLeg = { ...routing, stops: stops.map((p, i) => ({ ...p, distance: i ? 1 : 0 })) };
        expect(importPlan(JSON.stringify({ ...file, trip: { ...file.trip, routing: equalLeg } })).trip.routing?.stops).toEqual(equalLeg.stops);
    });

    it('rejects malformed plans, versions and picked routes without accepting part of a file', () => {
        const file = JSON.parse(exportPlan(newPlan(initialTrip(), 'Valid', [newVersion(initialTrip())])));
        for (const invalid of [null, {}, { ...file, version: 99 }, { ...file, trip: {} },
            { ...file, trip: { ...file.trip, startDate: '2026-02-30' } },
            { ...file, trip: { ...file.trip, points: [{ ...file.trip.points[0], anchor: [999, 0] }, file.trip.points[1]] } },
            { ...file, trip: { ...file.trip, routing: { picked: true, edges: {} } } },
            { ...file, versions: [null] }, { ...file, versions: [file.versions[0], file.versions[0]] },
            { ...file, versions: [{ ...file.versions[0], trip: {} }] }]) {
            expect(() => importPlan(JSON.stringify(invalid))).toThrow();
        }
        expect(() => importPlan('{')).toThrow();
    });
});
