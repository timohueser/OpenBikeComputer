import { afterEach, describe, expect, it, vi } from 'vitest';
import { calculateLine, profileId, requestAlternatives, selectRoute, movingSecondsAt, type EngineRoute, type RouteTotals } from './routing';
import { LegCache } from './route-legs';
import { routeService } from '../../../test-support/planner/route-service';
import { decodeRoutes, type AnswerRoute } from './route-answer';
import { ridingProfiles, presetName, type BikeType } from './riding-profiles';
import { surfaceRuns, surfaceWindow } from './surface-data';
import { initialTrip, cumulative, planOf, removeRoutePoint, storedPlan, setEndpoint, routingKey, routeCoordinates, TripHistory, type Coordinate, type Trip } from './editor';

const totals = (metres: number, unknown = 0, pushing = 0): RouteTotals =>
    ({ distance_m: metres, ascent_m: 0, seconds: metres, surface_m: [unknown, metres - unknown, 0, 0, 0, 0], unknown_elevation_m: metres, pushing_m: pushing });
const answer: AnswerRoute = {
    id: 'test-route', surfaces: [['Paved', 1], ['Gravel', 1]], pushing: [[false, 1], [true, 1]], closures: [[null, 2]], sac_scale: [[null, 2]], reason: 'primary', elapsed_s: [0, 3000, 1000], package: 'test', profile: 'touring',
    coordinates_udeg: [7_800_000, 48_000_000, 100_000, 0, 100_000, 0], elevation_dm: [2000, null, 2000],
    totals: { ...totals(15000, 1000, 100), seconds: 4000 },
    legs: [{ from_index: 0, to_index: 1, start: 'a', end: 'b', totals: { ...totals(7500, 1000), seconds: 3000 } },
        { from_index: 1, to_index: 2, start: 'b', end: 'c', totals: { ...totals(7500, 0, 100), seconds: 1000 } }],
    snap_truncated: false,
};
const [route] = decodeRoutes({ routes: [answer] });
function trip(): Trip {
    return { ...initialTrip(), live: true, points: [
        { id: 'start', kind: 'start', coordinate: [7.8, 48], label: 'Start', progress: 0 },
        { id: 'shape', kind: 'via', coordinate: [7.9, 48], label: 'Shape', progress: .5 },
        { id: 'finish', kind: 'finish', coordinate: [8, 48], label: 'End', progress: 1 },
    ] };
}
afterEach(() => vi.unstubAllGlobals());
describe('routing integration', () => {
    it('round-trips every rider-visible preset through the routing API identifier', () => {
        for (const [bike, profile] of Object.entries(ridingProfiles)) {
            for (const preset of profile.presets) {
                const id = profileId({ ...trip(), bike: bike as BikeType, preset });
                expect(id.split('/')[0]).toBe(bike);
                expect(presetName(id)).toBe(preset);
            }
        }
        expect(profileId({ ...trip(), bike: 'road', preset: 'Quieter' })).toBe('road/quieter');
    });
    it('keeps directed shaping context in one request and preserves unknown elevation', async () => {
        const fetch = vi.fn().mockResolvedValue({ ok: true, json: async () => ({ routes: [answer] }) });
        vi.stubGlobal('fetch', fetch);
        const plan = trip();
        const line = await calculateLine(plan, new AbortController().signal, new LegCache());
        expect(fetch).toHaveBeenCalledTimes(1);
        expect(JSON.parse(fetch.mock.calls[0][1].body).alternatives).toBe(false);
        expect(line.alternativesReady).toBe(false);
        expect(line.surfaces).toEqual(['Paved', 'Gravel']);
        expect(line.pushing).toEqual([false, true]);
        expect(line.package).toBe('test');
        expect(JSON.parse(fetch.mock.calls[0][1].body).points).toEqual(plan.points.map(p => p.coordinate));
        fetch.mockResolvedValueOnce({ ok: true, json: async () => ({ routes: [{ ...answer, id: 'shorter', reason: 'shorter' }] }) });
        const alternatives = await requestAlternatives(plan, line, new AbortController().signal);
        const { alternatives: _primaryOnly, ...request } = JSON.parse(fetch.mock.calls[0][1].body);
        expect(JSON.parse(fetch.mock.calls[1][1].body)).toEqual({ ...request, alternatives_only: true });
        expect(alternatives.map(r => r.id)).toEqual(['primary', 'shorter']);
        expect(movingSecondsAt(line, 0.5)).toBeCloseTo(3000);
        expect(line.elevation).toEqual([200, null, 400]);
        expect(line.stops.map(s => s.distance)).toEqual(cumulative(route.geometry));
        expect(routeCoordinates({ ...plan, routing: line })).toEqual(route.geometry);
    });
    it('keeps itinerary edits outside routing and preserves a selected saved line', () => {
        const plan = trip();
        plan.routing = selectRoute(plan, route, [route]);
        const edited = { ...plan, days: 5, splits: { 1: .4 }, restAfter: [1], limit: 10, points: plan.points.map(p => ({ ...p, label: 'Renamed' })) };
        expect(routingKey(edited)).toBe(routingKey(plan));
        expect(routeCoordinates(edited)).toEqual(route.geometry);
        const sameStart = setEndpoint(edited, 'start', edited.points[0].coordinate, 'New label');
        expect(routeCoordinates(sameStart)).toEqual(route.geometry);
        const marked = { ...edited, points: [...edited.points, { ...edited.points[0], id: 'note', kind: 'marker' as const }] };
        const unmarked = removeRoutePoint(marked, 'note');
        expect(unmarked.routing).toBe(edited.routing);
        expect(unmarked.splits).toEqual(edited.splits);
        const moved = { ...edited, points: edited.points.map(p => p.id === 'shape' ? { ...p, coordinate: [8.1, 48] as [number, number] } : p) };
        expect(routingKey(moved)).not.toBe(routingKey(plan));
        expect(routeCoordinates(moved)).toEqual([plan.points[0].coordinate]);
    });
    it('requests only the legs that an edit changes, pinned to the cached legs on both sides', async () => {
        const fetch = vi.fn<(url: string, init: RequestInit) => Promise<unknown>>(routeService('one'));
        vi.stubGlobal('fetch', fetch);
        const body = (call: number) => JSON.parse(fetch.mock.calls[call][1].body as string);
        const cache = new LegCache();
        const signal = new AbortController().signal;
        const plan: Trip = { ...trip(), points: [7.8, 7.85, 7.9, 7.95, 8].map((lon, i) => ({ id: `p${i}`, kind: i ? i < 4 ? 'via' : 'finish' : 'start', coordinate: [lon, 48], label: '', progress: i / 4 })) };
        const move = (lon: number): Trip => ({ ...plan, points: plan.points.map(p => p.id === 'p2' ? { ...p, coordinate: [lon, 48.01] } : p) });
        const coordinates = (trip: Trip) => trip.points.map(p => p.coordinate);
        const whole = await calculateLine(plan, signal, cache);
        expect(body(0)).toMatchObject({ points: coordinates(plan) });
        expect(body(0).start_position).toBeUndefined();
        const moved = await calculateLine(move(7.91), signal, cache);
        expect(body(1)).toMatchObject({ points: [[7.85, 48], [7.91, 48.01], [7.95, 48]], start_position: 'one:7.85,48', end_position: 'one:7.95,48' });
        expect(moved.coordinates).toEqual(coordinates(move(7.91)));
        expect([moved.seconds, moved.unknownSurfaceKm, moved.stops.length]).toEqual([4000, 0, 5]);
        expect((await calculateLine(plan, signal, cache)).coordinates).toEqual(whole.coordinates);
        expect(fetch).toHaveBeenCalledTimes(2);
        await calculateLine({ ...plan, bike: 'gravel' }, signal, cache);
        expect(body(2)).toMatchObject({ points: coordinates(plan), profile: 'gravel' });
        fetch.mockImplementation(routeService('two'));
        await calculateLine(move(7.92), signal, cache);
        expect(body(3).start_position).toBe('one:7.85,48');
        expect(body(4)).toMatchObject({ points: coordinates(move(7.92)) });
        expect(body(4).start_position).toBeUndefined();
        expect(fetch).toHaveBeenCalledTimes(5);
        // A pinned window without a path is followed by one request for the whole run.
        fetch.mockImplementationOnce(async () => ({ ok: false, json: async () => ({ code: 'no_path', message: 'No legal route.' }) }));
        expect((await calculateLine(move(7.93), signal, cache)).coordinates).toEqual(coordinates(move(7.93)));
        expect(body(5).start_position).toBe('two:7.85,48');
        expect(body(6)).toMatchObject({ points: coordinates(move(7.93)) });
        expect(body(6).start_position).toBeUndefined();
        fetch.mockImplementationOnce(async () => ({ ok: false, json: async () => ({ code: 'busy', message: 'Busy.' }) }));
        await expect(calculateLine(move(7.94), signal, cache)).rejects.toThrow('Busy.');
        expect(fetch).toHaveBeenCalledTimes(8);
    });
    it('keeps a picked corridor with its plan', () => {
        const plan = trip();
        const corridor: EngineRoute = { ...route, id: 'corridor', reason: 'corridor', geometry: [[7.8, 48], [7.9, 48.05], [8, 48]] };
        const primary = selectRoute(plan, route, [route, corridor]);
        const picked = { ...plan, routing: selectRoute(plan, corridor, [route, corridor]) };
        const history = new TripHistory();
        const shown = history.commit({ ...plan, routing: primary }, picked);
        expect(shown.routing?.choiceId).toBe('corridor');
        expect(planOf(shown)).toBe(picked);
        const undone = history.undo(shown);
        expect(undone.routing).toBeUndefined();
        expect(storedPlan(shown).routing).toMatchObject({ choiceId: 'corridor', picked: true, alternatives: [] });
        expect([shown.routing?.package, storedPlan(shown).routing?.package]).toEqual(['test', undefined]);
        expect(history.redo(undone).routing?.choiceId).toBe('corridor');
        expect(planOf({ ...picked, points: picked.points.map(p => p.id === 'shape' ? { ...p, coordinate: [7.95, 48] as Coordinate } : p) }).routing).toBeUndefined();
    });
    it('makes visit reversals explicit and accounts for manual joins', async () => {
        const visit = { ...answer, legs: [[0, 1], [1, 1], [1, 2], [2, 2]].map(([from_index, to_index], k) => ({ ...answer.legs[0], from_index, to_index, start: `${k}`, end: `${k + 1}` })) };
        const fetch = vi.fn().mockResolvedValueOnce({ ok: true, json: async () => ({ routes: [visit] }) })
            .mockResolvedValue({ ok: true, json: async () => ({ routes: [{ ...answer, legs: [{ ...answer.legs[0], to_index: 2 }] }] }) });
        vi.stubGlobal('fetch', fetch);
        const plan = trip();
        plan.points[1] = { ...plan.points[1], kind: 'detour', anchor: [7.85, 48] };
        await calculateLine(plan, new AbortController().signal, new LegCache());
        const body = JSON.parse(fetch.mock.calls[0][1].body);
        expect(body.points).toEqual([[7.8, 48], [7.85, 48], [7.9, 48], [7.85, 48], [8, 48]]);
        expect(body.turnarounds).toEqual([2]);
        expect(body.alternatives).toBe(false);
        const manual = trip();
        manual.points[1].leg = 'straight';
        manual.points[2].leg = 'straight';
        const line = await calculateLine(manual, new AbortController().signal, new LegCache());
        expect(line.unroutedKm).toBeCloseTo(line.unknownSurfaceKm);
        expect(line.elapsed.at(-1)).toBe(line.seconds);
        expect(line.elevation.every(h => h === null)).toBe(true);
        expect(line.surfaces).toHaveLength(line.coordinates.length - 1);
        expect(line.surfaces.every(s => s === 'Unknown')).toBe(true);
        expect(line.pushing).toEqual([null, null]);
        manual.points[2].leg = 'routed';
        const mixed = await calculateLine(manual, new AbortController().signal, new LegCache());
        expect(mixed.surfaces).toEqual(['Unknown', 'Unknown', 'Paved', 'Gravel']);
        expect(mixed.surfaces).toHaveLength(mixed.coordinates.length - 1);
        expect(mixed.pushing).toEqual([null, null, false, true]);
    });
    it('aligns surface sections by distance and clips the view without changing route shares', () => {
        const line = selectRoute(trip(), route, [route]);
        const data = surfaceRuns(line);
        expect(data.runs.map(run => run.surface)).toEqual(['Paved', 'Gravel']);
        expect(data.runs[0].to).toBeCloseTo(.5);
        expect(data.runs[1].from).toBe(data.runs[0].to);
        const clipped = surfaceWindow(data.runs, .25, .75);
        expect(clipped[0]).toEqual({ ...data.runs[0], from: .25 });
        expect(clipped[1]).toEqual({ ...data.runs[1], to: .75 });
        expect(data.shares.get('Gravel')).toBeCloseTo(.5);
        expect(surfaceRuns({ ...line, surfaces: [] }).shares.get('Unknown')).toBe(1);
        expect(surfaceRuns().runs).toEqual([]);
        const sameSurface = surfaceRuns({ ...line, surfaces: ['Paved', 'Paved'] });
        expect(sameSurface.runs.map(run => run.pushing)).toEqual([false, true]);
        expect(sameSurface.shares.get('Paved')).toBe(1);
    });
    it('reports service failure and never substitutes fixture geometry', async () => {
        vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: false, json: async () => ({ code: 'no_path', message: 'No legal route.' }) }));
        const plan = trip();
        await expect(calculateLine(plan, new AbortController().signal, new LegCache())).rejects.toThrow('No legal route.');
        expect(routeCoordinates(plan)).toEqual([plan.points[0].coordinate]);
    });
});
