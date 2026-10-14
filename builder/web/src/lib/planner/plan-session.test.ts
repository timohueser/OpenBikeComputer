import { IDBFactory } from 'fake-indexeddb';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { emptyTrip, orderedRoutePoints, type RoutePoint, type Trip } from './editor';
import type { Coordinate } from './geo';
import { PlanLibrary } from './library';
import { PlanSession } from './plan-session.svelte';
import { newVersion } from './versions';
import type { AnswerRoute } from './route-answer';
import { routeService } from '../../../test-support/planner/route-service';

const point = (id: string, kind: RoutePoint['kind'], coordinate: Coordinate): RoutePoint => ({ id, kind, label: id, coordinate });
const plan: Trip = { ...emptyTrip(), points: [point('start', 'start', [8, 48]), point('finish', 'finish', [8.1, 48])] };
const moved = (trip: Trip, lon: number): Trip => ({ ...trip, points: trip.points.map(p => p.id === 'finish' ? { ...p, coordinate: [lon, 48] } : p) });

/** The stub route service, its request bodies, and a switch that holds the next answers until `release`. Address search
 * answers `reverse`, which finds no address unless a test sets one. */
function service(alternatives: AnswerRoute[] = []) {
    const answer = routeService();
    let held: Promise<void> | undefined, release = () => {};
    const fetch = vi.fn(async (url: string, init: RequestInit): Promise<unknown> => {
        await held;
        const body = JSON.parse(init.body as string);
        return body.alternatives_only ? { ok: true, json: async () => ({ routes: alternatives }) } : answer(url, init);
    });
    const reverse = vi.fn(async (_url: string, _init: RequestInit): Promise<unknown> => ({ ok: true, json: async () => ({ label: null }) }));
    vi.stubGlobal('fetch', (url: string, init: RequestInit) => (url.endsWith('/reverse') ? reverse : fetch)(url, init));
    return {
        fetch,
        reverse,
        bodies: () => fetch.mock.calls.map(([, init]) => JSON.parse(init.body as string)),
        hold() { held = new Promise(resolve => release = () => { held = undefined; resolve(); }); },
        release: () => release(),
    };
}

/** A route of the alternatives answer through `line`, as one leg. */
function alternative(id: string, reason: string, profile: string, line: Coordinate[], via?: Coordinate): AnswerRoute {
    const totals = { distance_m: 9000, ascent_m: 0, seconds: 900, surface_m: [0, 9000, 0, 0, 0, 0], unknown_elevation_m: 0, pushing_m: 0 };
    return { id, reason, package: 'test', profile, snap_truncated: false, totals, ...via ? { via } : {},
        coordinates_udeg: line.flat().map((value, i, flat) => Math.round(value * 1e6) - (i > 1 ? Math.round(flat[i - 2] * 1e6) : 0)),
        elevation_dm: line.map(() => null), elapsed_s: line.map((_, i) => i && 300),
        legs: [{ from_index: 0, to_index: line.length - 1, start: 'a', end: 'b', totals }] };
}

function options(session: PlanSession) {
    const options = session.routeOptions;
    if (options.state !== 'ready') throw new Error(`Route options are ${options.state}.`);
    return options;
}
const choice = (session: PlanSession) => session.routeOptions.state === 'ready' ? session.routeOptions.choice : undefined;

afterEach(() => vi.unstubAllGlobals());

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

    it('keeps the alternatives through picks: a corridor is one shaping point, a profile the preset, and its legs need no request', async () => {
        const via: Coordinate = [8.05, 48.02];
        const routes = service([alternative('corridor', 'corridor', 'touring', [[8, 48], via, [8.1, 48]], via),
            alternative('shorter', 'shorter', 'touring/shorter', [[8, 48], [8.1, 48]])]);
        const session = new PlanSession();
        session.commit(plan);
        await vi.waitFor(() => expect(session.line).toBeDefined());
        session.findAlternatives();
        await vi.waitFor(() => expect(choice(session)).toBe('primary'));
        const [primary, corridor, shorter] = options(session).routes;
        expect(routes.bodies().map(body => [body.points, body.alternatives_only])).toEqual([[[[8, 48], [8.1, 48]], undefined], [[[8, 48], [8.1, 48]], true]]);
        const pick = async (route: typeof primary) => {
            expect(session.pickAlternative(route)).toBe(true);
            expect(choice(session)).toBe(route.id);
            await vi.waitFor(() => expect(session.line).toBeDefined());
        };
        await pick(shorter);
        expect(session.trip).toMatchObject({ preset: 'Shorter', points: plan.points });
        expect(routes.fetch).toHaveBeenCalledTimes(2);
        await pick(corridor);
        expect(session.trip.preset).toBe('Balanced');
        expect(orderedRoutePoints(session.trip).map(p => [p.kind, p.coordinate])).toEqual([['start', [8, 48]], ['via', via], ['finish', [8.1, 48]]]);
        expect(routes.bodies().at(-1)).toMatchObject({ points: [[8, 48], via, [8.1, 48]], profile: 'touring' });
        for (const route of [shorter, corridor, primary]) await pick(route);
        expect(session.trip).toMatchObject({ preset: 'Balanced', points: plan.points });
        expect(routes.fetch).toHaveBeenCalledTimes(3);
        // The checked route adds no Undo step: one Undo returns to the corridor.
        expect(session.pickAlternative(primary)).toBe(false);
        expect(session.undo() && choice(session)).toBe('corridor');
        expect(routes.fetch).toHaveBeenCalledTimes(3);
    });

    it('previews an option without an edit, applies it as one Undo step, and refuses options of an earlier plan', async () => {
        const via: Coordinate = [8.05, 48.02];
        const routes = service([alternative('corridor', 'corridor', 'touring', [[8, 48], via, [8.1, 48]], via)]);
        const session = new PlanSession();
        await session.start(new IDBFactory());
        session.commit(plan);
        await vi.waitFor(() => expect(session.line).toBeDefined());
        session.findAlternatives();
        await vi.waitFor(() => expect(choice(session)).toBe('primary'));
        const [primary, corridor] = options(session).routes;
        await session.saved();
        const save = vi.spyOn(session, 'save');
        const before = session.trip;

        session.previewOption(corridor.id);
        expect([session.compared, session.trip, session.canRedo]).toEqual([corridor, before, false]);
        session.previewOption(primary.id);
        expect(session.compared).toBeUndefined();
        session.previewOption(corridor.id);
        session.previewOption(null);
        expect([session.compared, session.trip]).toEqual([undefined, before]);
        expect(save).not.toHaveBeenCalled();

        session.previewOption(corridor.id);
        expect(session.pickAlternative(corridor)).toBe(true);
        expect([session.compared, choice(session), save.mock.calls.length]).toEqual([undefined, 'corridor', 1]);
        expect(session.undo() && session.trip).toBe(before);

        // An edit makes the options stale; they never apply to the edited plan.
        session.previewOption(corridor.id);
        session.commit(moved(plan, 8.2));
        expect([session.routeOptions.state, session.compared]).toEqual(['stale', undefined]);
        expect(session.pickAlternative(corridor)).toBe(false);
        await vi.waitFor(() => expect(session.line).toBeDefined());
        routes.fetch.mockResolvedValueOnce({ ok: false, json: async () => ({ message: 'Routing is unavailable. Retry shortly.' }) });
        session.findAlternatives();
        await vi.waitFor(() => expect(session.routeOptions).toEqual({ state: 'error', message: 'Routing is unavailable. Retry shortly.' }));
        session.findAlternatives();
        await vi.waitFor(() => expect(choice(session)).toBe('primary'));
        expect(session.pickAlternative(corridor)).toBe(false);
        session.close();
    });

    it('names a corridor option after the settlement of the address nearest its via', async () => {
        const via: Coordinate = [8.05, 48.02];
        const routes = service([alternative('corridor', 'corridor', 'touring', [[8, 48], via, [8.1, 48]], via)]);
        routes.reverse.mockResolvedValueOnce({ ok: true, json: async () => ({ label: 'Dietenbacher Straße 30, Kirchzarten' }) });
        const session = new PlanSession();
        session.commit(plan);
        await vi.waitFor(() => expect(session.line).toBeDefined());
        session.findAlternatives();
        await vi.waitFor(() => expect(session.optionPlaces).toEqual({ corridor: 'Kirchzarten' }));
        expect(JSON.parse(routes.reverse.mock.calls[0][1].body as string)).toEqual({ coordinate: via });
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
});
