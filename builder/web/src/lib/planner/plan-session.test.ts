import { IDBFactory } from 'fake-indexeddb';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { emptyTrip, orderedRoutePoints, setDays, type RoutePoint, type Trip } from './editor';
import type { Coordinate } from './geo';
import { Library } from '../library/store';
import { PlanSession } from './plan-session.svelte';
import { newVersion } from './versions';
import * as routing from './routing';
import { routingKey } from './routing';
import type { AnswerRoute } from './route-answer';
import { routeService } from '../../../test-support/planner/route-service';
import { testLine } from '../../../test-support/planner/trip';

const point = (id: string, kind: RoutePoint['kind'], coordinate: Coordinate): RoutePoint => ({ id, kind, label: id, coordinate });
// A route whose day count the rider set, so the planner does not guess it.
const plan: Trip = { ...emptyTrip(), daysSet: true, points: [point('start', 'start', [8, 48]), point('finish', 'finish', [8.1, 48])] };
const moved = (trip: Trip, lon: number): Trip => ({ ...trip, points: trip.points.map(p => p.id === 'finish' ? { ...p, coordinate: [lon, 48] } : p) });

/** The stub route service, its request bodies, and a switch that holds the next answers until `release`. Address search
 * answers `places`, which finds no place unless a test sets one. */
function service(alternatives: AnswerRoute[] = []) {
    const answer = routeService();
    let held: Promise<void> | undefined, release = () => {};
    const fetch = vi.fn(async (url: string, init: RequestInit): Promise<unknown> => {
        await held;
        const body = JSON.parse(init.body as string);
        return body.alternatives_only ? { ok: true, json: async () => ({ routes: alternatives }) } : answer(url, init);
    });
    const places = vi.fn(async (_url: string, _init: RequestInit): Promise<unknown> => ({ ok: true, json: async () => ({ type: 'places', results: [] }) }));
    vi.stubGlobal('fetch', (url: string, init: RequestInit) => (url.endsWith('/query') ? places : fetch)(url, init));
    return {
        fetch,
        places,
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

afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); });

const active = async (library: Library) => {
    const id = await library.state('active');
    return id ? library.route(id) : undefined;
};

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
            alternative('climb', 'less_climbing', 'touring/less-climbing', [[8, 48], [8.1, 48]])]);
        const session = new PlanSession();
        session.commit(plan);
        await vi.waitFor(() => expect(session.line).toBeDefined());
        session.findAlternatives();
        await vi.waitFor(() => expect(choice(session)).toBe('primary'));
        const [primary, corridor, lower] = options(session).routes;
        expect(routes.bodies().map(body => [body.points, body.alternatives_only])).toEqual([[[[8, 48], [8.1, 48]], undefined], [[[8, 48], [8.1, 48]], true]]);
        const pick = async (route: typeof primary) => {
            expect(session.pickAlternative(route)).toBe(true);
            expect(choice(session)).toBe(route.id);
            await vi.waitFor(() => expect(session.line).toBeDefined());
        };
        await pick(lower);
        expect(session.trip).toMatchObject({ preset: 'Less climbing', points: plan.points });
        expect(routes.fetch).toHaveBeenCalledTimes(2);
        await pick(corridor);
        expect(session.trip.preset).toBe('Balanced');
        expect(orderedRoutePoints(session.trip).map(p => [p.kind, p.coordinate])).toEqual([['start', [8, 48]], ['via', via], ['finish', [8.1, 48]]]);
        expect(routes.bodies().at(-1)).toMatchObject({ points: [[8, 48], via, [8.1, 48]], profile: 'touring' });
        for (const route of [lower, corridor, primary]) await pick(route);
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
        await session.start(new Library(new IDBFactory()));
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

    it('keeps the options of a plan that Undo returns to during a request for the edit, and offers no corridor without via', async () => {
        const via: Coordinate = [8.05, 48.02];
        const routes = service([alternative('corridor', 'corridor', 'touring', [[8, 48], via, [8.1, 48]], via),
            alternative('bare', 'corridor', 'touring', [[8, 48], [8.05, 47.98], [8.1, 48]])]);
        const session = new PlanSession();
        session.commit(plan);
        await vi.waitFor(() => expect(session.line).toBeDefined());
        session.findAlternatives();
        await vi.waitFor(() => expect(choice(session)).toBe('primary'));
        expect(options(session).routes.map(route => route.id)).toEqual(['primary', 'corridor']);
        session.commit(moved(plan, 8.2));
        await vi.waitFor(() => expect(session.line).toBeDefined());
        routes.hold();
        session.findAlternatives();
        session.undo();
        session.findAlternatives();
        session.previewOption('corridor');
        routes.release();
        await routes.fetch.mock.results.at(-1)!.value;
        await new Promise(resolve => setTimeout(resolve));
        expect([choice(session), session.compared?.id]).toEqual(['primary', 'corridor']);
    });

    it('names a corridor option after the nearest city, town or village within 3 km of its via', async () => {
        const via: Coordinate = [8.05, 48.02];
        const routes = service([alternative('corridor', 'corridor', 'touring', [[8, 48], via, [8.1, 48]], via)]);
        const place = (name: string, kind: string, distance: number) => ({ name, kind, distance });
        routes.places.mockResolvedValueOnce({ ok: true, json: async () => ({ type: 'places',
            results: [place('Oberneuhäuser', 'hamlet', .4), place('Kirchzarten', 'village', .8), place('Freiburg', 'city', 2.9)] }) });
        const session = new PlanSession();
        session.commit(plan);
        await vi.waitFor(() => expect(session.line).toBeDefined());
        session.findAlternatives();
        await vi.waitFor(() => expect(session.optionPlaces).toEqual({ corridor: 'Kirchzarten' }));
        const { view, request } = JSON.parse(routes.places.mock.calls[0][1].body as string);
        expect([(view[0] + view[2]) / 2, (view[1] + view[3]) / 2].map(n => n.toFixed(6))).toEqual(via.map(n => n.toFixed(6)));
        expect(request).toEqual({ type: 'places', what: ['town'], radius: { value: 3, unit: 'km' } });
    });

    it('calculates each saved version once, a failed one again on retry, and opens a version with its line', async () => {
        const routes = service();
        const session = new PlanSession();
        await session.start(new Library(new IDBFactory()));
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
        await session.start(new Library(new IDBFactory()));
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
        await session.start(new Library(factory));
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
        expect((await active(new Library(factory)))?.open).toBe(b);
        session.close();
    });

    it('keeps the earlier steps with the version saved from them, so Undo makes no copies and no empty version', async () => {
        service();
        const session = new PlanSession();
        await session.start(new Library(new IDBFactory()));
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
        await session.start(new Library(new IDBFactory()));
        session.commit(plan);
        await session.saveVersions([newVersion(plan, undefined, 'Valley')]);
        const first = session.plan;
        expect(await session.newPlan()).toBe(true);
        expect(session.versions).toEqual([]);
        expect(await session.duplicate(first)).toBe(true);
        expect([session.plan.id === first.id, session.versions.map(v => v.name)]).toEqual([false, ['Valley']]);
        session.close();
    });

    it('saves the plan with the summary of its line, and calculates the line again when the plan opens', async () => {
        const routes = service();
        const factory = new IDBFactory();
        const first = new PlanSession();
        await first.start(new Library(factory));
        first.commit(plan);
        const library = new Library(factory);
        await vi.waitFor(async () => expect((await active(library))?.summary).toBeTruthy());
        const saved = (await active(library))!;
        expect(saved.trip).toEqual(plan);
        expect(saved.summary).toMatchObject({ line: [8, 48, 8.1, 48], days: 1, bike: 'touring', bounds: [8, 48, 8.1, 48] });
        expect(saved.summary!.lengthM).toBeCloseTo(7440, -2);
        first.close();
        const opened = vi.fn();
        const second = new PlanSession(opened);
        await second.start(new Library(factory));
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
});
