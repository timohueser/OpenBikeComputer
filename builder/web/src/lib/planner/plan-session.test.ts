import { IDBFactory } from 'fake-indexeddb';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { emptyTrip, orderedRoutePoints, planTitle, setDays, type RoutePoint, type Trip } from './editor';
import type { Coordinate } from './geo';
import { PlanLibrary } from './library';
import { PlanSession } from './plan-session.svelte';
import { newVersion } from './versions';
import * as routing from './routing';
import { routingKey } from './routing';
import { routeService } from '../../../test-support/planner/route-service';
import { testLine } from '../../../test-support/planner/trip';

const point = (id: string, kind: RoutePoint['kind'], coordinate: Coordinate): RoutePoint => ({ id, kind, label: id, coordinate });
// A route whose day count the rider set, so the planner does not guess it.
const plan: Trip = { ...emptyTrip(), daysSet: true, points: [point('start', 'start', [8, 48]), point('finish', 'finish', [8.1, 48])] };
const moved = (trip: Trip, lon: number): Trip => ({ ...trip, points: trip.points.map(p => p.id === 'finish' ? { ...p, coordinate: [lon, 48] } : p) });

/** The stub route service and a switch that holds the next answers until `release`. */
function service() {
    const answer = routeService();
    let held: Promise<void> | undefined, release = () => {};
    const fetch = vi.fn(async (url: string, init: RequestInit): Promise<unknown> => {
        await held;
        return answer(url, init);
    });
    vi.stubGlobal('fetch', fetch);
    return {
        fetch,
        hold() { held = new Promise(resolve => release = () => { held = undefined; resolve(); }); },
        release: () => release(),
    };
}

afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); });

describe('plan session', () => {
    it('shows the last routed trip while a line is calculated, no line after a failure, and undoes without a request', async () => {
        const routes = service();
        const session = new PlanSession();
        session.commit(plan);
        await vi.waitFor(() => expect(session.line).toBeDefined());
        const first = session.line;
        routes.hold();
        session.commit(moved(plan, 8.2));
        expect([session.stale, session.shown.trip, session.shown.line]).toEqual([true, plan, first]);
        routes.release();
        await vi.waitFor(() => expect(session.stale).toBe(false));
        expect(session.shown.trip).toBe(session.trip);
        expect(session.shown.line?.coordinates.at(-1)).toEqual([8.2, 48]);
        routes.fetch.mockResolvedValueOnce({ ok: false, json: async () => ({ code: 'no_path', message: 'No legal route.' }) });
        session.commit(moved(plan, 8.3));
        await vi.waitFor(() => expect(session.routeError).toBe('No legal route.'));
        expect([session.stale, session.shown.line]).toEqual([false, undefined]);
        session.retry();
        await vi.waitFor(() => expect(session.line).toBeDefined());
        const requests = routes.fetch.mock.calls.length;
        expect([session.undo(), session.undo(), session.canUndo, session.canRedo]).toEqual([true, true, true, true]);
        expect([session.trip, session.line]).toEqual([plan, first]);
        expect(session.redo()).toBe(true);
        expect(session.line?.coordinates.at(-1)).toEqual([8.2, 48]);
        expect(routes.fetch).toHaveBeenCalledTimes(requests);
    });

    it('calculates each saved version once, a failed one again on retry, and opens a version with its line', async () => {
        const routes = service();
        const session = new PlanSession();
        await session.start(new IDBFactory());
        session.commit(plan);
        await vi.waitFor(() => expect(session.line).toBeDefined());
        const valley = moved(plan, 8.2), ridge = moved(plan, 8.3);
        await session.saveVersions([newVersion(valley, undefined, 'Valley'), newVersion(ridge, undefined, 'Ridge')]);
        routes.fetch.mockResolvedValueOnce({ ok: false, json: async () => ({ code: 'no_path', message: 'No legal route.' }) });
        session.findVersionLines();
        await vi.waitFor(() => expect(Object.values(session.versionLines)).toHaveLength(2));
        expect(Object.values(session.versionLines).filter(line => line === 'No legal route.')).toHaveLength(1);
        const requests = routes.fetch.mock.calls.length;
        session.findVersionLines();
        expect(routes.fetch).toHaveBeenCalledTimes(requests);
        session.findVersionLines(true);
        await vi.waitFor(() => expect(Object.values(session.versionLines).every(line => typeof line === 'object')).toBe(true));
        expect(routes.fetch).toHaveBeenCalledTimes(requests + 1);
        expect(await session.openVersion(session.versions.find(v => v.name === 'Valley')!.id)).toBe(true);
        expect(session.line).toBe(session.versionLines[routingKey(valley)]);
        expect(routes.fetch).toHaveBeenCalledTimes(requests + 1);
        session.close();
    });

    it('keeps the edits of the open version when another opens, and saves a plan of no version first', async () => {
        service();
        const session = new PlanSession();
        await session.start(new IDBFactory());
        session.commit(plan);
        await session.saveNewVersion('A');
        const a = session.openId!;
        session.commit(moved(plan, 8.2));
        await session.saveNewVersion('B');
        const b = session.openId!;
        session.commit(moved(plan, 8.3));
        expect(await session.openVersion(b)).toBe(true);
        expect(session.trip).toEqual(moved(plan, 8.3));
        expect(await session.openVersion(a)).toBe(true);
        expect([session.trip, session.openId]).toEqual([plan, a]);
        session.commit(moved(plan, 8.4));
        expect(await session.openVersion(b)).toBe(true);
        expect([session.trip, session.openId]).toEqual([moved(plan, 8.3), b]);
        expect(await session.openVersion(a)).toBe(true);
        expect(session.trip).toEqual(moved(plan, 8.4));
        expect(session.versions.map(v => [v.id, v.name]).sort()).toEqual([[a, 'A'], [b, 'B']].sort());

        await session.saveVersions(session.versions.filter(v => v.id !== a));
        expect(session.openId).toBeUndefined();
        session.commit(moved(plan, 8.5));
        expect(await session.openVersion(b)).toBe(true);
        expect(session.versions.map(v => [v.name, v.trip])).toEqual([[undefined, moved(plan, 8.5)], ['B', moved(plan, 8.3)]]);
        session.close();
    });

    it('undoes a switch of versions in one step, with the open version from before', async () => {
        service();
        const factory = new IDBFactory();
        const session = new PlanSession();
        await session.start(factory);
        session.commit(plan);
        await session.saveNewVersion('A');
        const a = session.openId;
        session.commit(moved(plan, 8.2));
        await session.saveNewVersion('B');
        const b = session.openId;
        expect(await session.openVersion(a!)).toBe(true);
        expect(session.undo()).toBe(true);
        expect([session.trip, session.openId]).toEqual([moved(plan, 8.2), b]);
        await session.saved();
        expect((await new PlanLibrary(factory).active())?.open).toBe(b);
        session.close();
    });

    it('keeps the earlier steps with the version saved from them, so Undo makes no copies and no empty version', async () => {
        service();
        const session = new PlanSession();
        await session.start(new IDBFactory());
        session.commit(plan);
        session.commit(moved(plan, 8.2));
        await session.saveNewVersion('A');
        const a = session.openId;
        expect(session.undo()).toBe(true);
        expect([session.trip, session.openId]).toEqual([plan, a]);

        expect(session.undo()).toBe(true);
        const b = newVersion(moved(plan, 8.9), undefined, 'B');
        await session.saveVersions([b, ...session.versions]);
        expect(await session.openVersion(b.id)).toBe(true);
        expect(session.versions.find(v => v.id === a)?.trip).toEqual(moved(plan, 8.2));

        const c = newVersion(moved(plan, 8.8), undefined, 'C');
        await session.saveVersions([c]);
        expect(session.openId).toBeUndefined();
        expect(await session.openVersion(c.id)).toBe(true);
        expect(session.undo()).toBe(true);
        expect(await session.openVersion(c.id)).toBe(true);
        expect(session.versions.map(v => v.name)).toEqual([undefined, 'C']);
        session.close();
    });

    it('duplicates a plan that is not open with the versions saved for it', async () => {
        service();
        const session = new PlanSession();
        await session.start(new IDBFactory());
        session.commit(plan);
        await session.saveVersions([newVersion(plan, undefined, 'Valley')]);
        const first = session.plan;
        expect(await session.newPlan()).toBe(true);
        expect(session.versions).toEqual([]);
        expect(await session.duplicate(first)).toBe(true);
        expect([session.plan.id === first.id, session.versions.map(v => v.name)]).toEqual([false, ['Valley']]);
        session.close();
    });

    it('saves the plan without its line and calculates the line again when the plan opens', async () => {
        const routes = service();
        const factory = new IDBFactory();
        const first = new PlanSession();
        await first.start(factory);
        first.commit(plan);
        const library = new PlanLibrary(factory);
        await vi.waitFor(async () => expect((await library.active())?.summary).toMatch(/^Single route · \d+\.\d km$/));
        expect((await library.active())?.trip).toEqual(plan);
        first.close();
        const opened = vi.fn();
        const second = new PlanSession(opened);
        await second.start(factory);
        expect(opened).toHaveBeenCalledOnce();
        expect([second.trip, second.canUndo]).toEqual([plan, false]);
        await vi.waitFor(() => expect(second.line).toBeDefined());
        expect(routes.fetch).toHaveBeenCalledTimes(2);
        await library.close();
    });

    it('guesses the day count of each new line in the Undo step of the edit, with one save, until the rider sets it', async () => {
        // Riding hours by the number of route points.
        const hours: Record<number, number> = { 2: 25, 3: 40, 4: 60 };
        vi.spyOn(routing, 'calculateLine').mockImplementation(async trip => {
            const line = testLine(trip), seconds = hours[orderedRoutePoints(trip).length] * 3600;
            return { ...line, elapsed: line.elapsed.map(s => s * seconds / line.seconds), seconds };
        });
        const session = new PlanSession(), saved: number[] = [];
        vi.spyOn(session, 'save').mockImplementation(async () => { saved.push(session.trip.days); });
        const visit = (trip: Trip, id: string): Trip => ({ ...trip, routeOrder: [...trip.routeOrder, id], points: [...trip.points, point(id, 'waypoint', [8.05, 48.1])] });
        session.commit({ ...plan, daysSet: undefined });
        await vi.waitFor(() => expect(session.trip.days).toBe(4));
        session.commit(visit(session.trip, 'a'));
        // A step of the same route before its line arrives.
        session.commit({ ...session.trip, stay: ['camp'] });
        await vi.waitFor(() => expect(session.trip.days).toBe(6));
        expect(saved).toEqual([3, 4, 4, 4, 6]);
        expect(session.undo()).toBe(true);
        expect([session.trip.routeOrder, session.trip.stay, session.trip.days]).toEqual([['a'], undefined, 6]);
        expect(session.undo()).toBe(true);
        expect([session.trip.routeOrder, session.trip.days]).toEqual([[], 4]);
        session.redo(); session.redo();
        session.commit(setDays(session.trip, 5));
        session.commit(visit(session.trip, 'b'));
        await vi.waitFor(() => expect(session.line).toBeDefined());
        expect(session.trip).toMatchObject({ days: 5, daysSet: true });
    });

    it('names an automatic endpoint in the open step only while it stays put, and unnames it when it moves', () => {
        vi.spyOn(routing, 'calculateLine').mockImplementation(async trip => testLine(trip));
        const session = new PlanSession();
        vi.spyOn(session, 'save').mockResolvedValue();
        session.commit({ ...plan, points: plan.points.map(p => p.id === 'start' ? { ...p, label: 'Start', autoLabel: true } : p) });
        session.autoName('start', [8, 48], 'Sempach');
        // A late answer for an earlier position, and a name for a point that the rider named.
        session.autoName('start', [7.9, 48], 'Elsewhere');
        session.autoName('finish', [8.1, 48], 'Elsewhere');
        expect(planTitle(session.trip)).toBe('Sempach → finish');
        expect([session.undo(), session.trip.points, session.redo(), planTitle(session.trip)]).toEqual([true, [], true, 'Sempach → finish']);
        session.movePoint('start', [8.02, 48]);
        session.movePoint('finish', [8.2, 48]);
        expect(session.trip.points.map(p => [p.label, p.autoLabel])).toEqual([['Start', true], ['finish', undefined]]);
        session.close();
    });
});
