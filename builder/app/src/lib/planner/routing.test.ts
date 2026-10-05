import { afterEach, describe, expect, it, vi } from 'vitest';
import { calculateLine, profileId, regionProfiles, requestAlternatives, requestRoute, routingKey, movingSecondsAt, type RouteTotals } from './routing';
import { LegCache } from './route-legs';
import { routeService } from '../../../test-support/planner/route-service';
import { decodeRoutes, type AnswerRoute } from './route-answer';
import { presetName, presetNames, servedPresets, type BikeType } from './riding-profiles';
import { surfaceRuns, surfaceWindow } from './surface-data';
import { planView, removeRoutePoint, setEndpoint, type RoutePoint, type Trip } from './editor';
import { cumulative, type Coordinate } from './geo';
import { testTrip } from '../../../test-support/planner/trip';

const totals = (metres: number, unknown = 0, pushing = 0): RouteTotals =>
    ({ distance_m: metres, ascent_m: 0, seconds: metres, surface_m: [unknown, metres - unknown, 0, 0, 0, 0], unknown_elevation_m: metres, pushing_m: pushing });
const answer: AnswerRoute = {
    id: 'test-route', edges: { surfaces: [['Paved', 1], ['Gravel', 1]], pushing: [[false, 1], [true, 1]] }, reason: 'primary', elapsed_s: [0, 3000, 1000], package: 'test', profile: 'touring',
    coordinates_udeg: [7_800_000, 48_000_000, 100_000, 0, 100_000, 0], elevation_dm: [2000, null, 2000],
    totals: { ...totals(15000, 1000, 100), seconds: 4000 },
    legs: [{ from_index: 0, to_index: 1, start: 'a', end: 'b', totals: { ...totals(7500, 1000), seconds: 3000 } },
        { from_index: 1, to_index: 2, start: 'b', end: 'c', totals: { ...totals(7500, 0, 100), seconds: 1000 } }],
    snap_truncated: false,
};
const [route] = decodeRoutes({ routes: [answer] });
function trip(): Trip {
    return { ...testTrip(), routeOrder: ['shape'], points: [
        { id: 'start', kind: 'start', coordinate: [7.8, 48], label: 'Start' },
        { id: 'shape', kind: 'via', coordinate: [7.9, 48], label: 'Shape' },
        { id: 'finish', kind: 'finish', coordinate: [8, 48], label: 'End' },
    ] };
}
afterEach(() => vi.unstubAllGlobals());
describe('routing integration', () => {
    it('offers the presets of the profiles that the region serves, each with its own profile ID', async () => {
        const profiles = ['gravel', 'gravel/shorter', 'hiking/less-climbing'];
        vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: true, json: async () => ({ package: 'test', profiles }) }));
        const served = await regionProfiles();
        expect([servedPresets('gravel', served), servedPresets('hiking', served), servedPresets('road', served), servedPresets('road', undefined)])
            .toEqual([['Balanced', 'Shorter'], ['Less climbing'], [], presetNames]);
        for (const id of profiles) expect(profileId({ ...trip(), bike: id.split('/')[0] as BikeType, preset: presetName(id) })).toBe(id);
        expect(profileId({ ...trip(), bike: 'road', preset: 'Quieter' })).toBe('road');
    });
    it('keeps directed shaping context in one request and preserves unknown elevation', async () => {
        const fetch = vi.fn().mockResolvedValue({ ok: true, json: async () => ({ routes: [answer] }) });
        vi.stubGlobal('fetch', fetch);
        const plan = trip();
        const line = await calculateLine(plan, new AbortController().signal, new LegCache());
        expect(fetch).toHaveBeenCalledTimes(1);
        expect(JSON.parse(fetch.mock.calls[0][1].body).alternatives).toBe(false);
        expect(line.primary?.geometry).toEqual(route.geometry);
        expect(line.edges).toEqual({ surfaces: ['Paved', 'Gravel'], pushing: [false, true] });
        expect(line.package).toBe('test');
        expect(JSON.parse(fetch.mock.calls[0][1].body).points).toEqual(plan.points.map(p => p.coordinate));
        fetch.mockResolvedValueOnce({ ok: true, json: async () => ({ routes: [{ ...answer, id: 'shorter', reason: 'shorter' }] }) });
        const alternatives = await requestAlternatives(plan, line.primary!, new AbortController().signal);
        const { alternatives: _primaryOnly, ...request } = JSON.parse(fetch.mock.calls[0][1].body);
        expect(JSON.parse(fetch.mock.calls[1][1].body)).toEqual({ ...request, alternatives_only: true });
        expect(alternatives.map(r => r.id)).toEqual(['primary', 'shorter']);
        expect(movingSecondsAt(line, 0.5)).toBeCloseTo(3000);
        expect(line.elevation).toEqual([200, null, 400]);
        expect(line.stops.map(s => s.distance)).toEqual(cumulative(route.geometry));
        expect(planView(plan, line).coordinates).toEqual(route.geometry);
    });
    it('keeps itinerary edits, labels and markers outside the routing key', () => {
        const plan = trip();
        const edited = { ...plan, days: 5, splits: { 1: .4 }, restAfter: [1], limit: 10, points: plan.points.map(p => ({ ...p, label: 'Renamed' })) };
        expect(routingKey(edited)).toBe(routingKey(plan));
        expect(routingKey(setEndpoint(edited, 'start', edited.points[0].coordinate, 'New label'))).toBe(routingKey(plan));
        const marked = { ...edited, points: [...edited.points, { ...edited.points[0], id: 'note', kind: 'marker' as const }] };
        const unmarked = removeRoutePoint(marked, 'note');
        expect([routingKey(marked), routingKey(unmarked)]).toEqual([routingKey(plan), routingKey(plan)]);
        expect(unmarked.splits).toEqual(edited.splits);
        const moved = { ...edited, points: edited.points.map(p => p.id === 'shape' ? { ...p, coordinate: [8.1, 48] as [number, number] } : p) };
        expect(routingKey(moved)).not.toBe(routingKey(plan));
        expect(routingKey({ ...plan, preset: 'Shorter' })).not.toBe(routingKey(plan));
    });
    it('requests only the legs that an edit changes, pinned to the cached legs on both sides', async () => {
        const fetch = vi.fn<(url: string, init: RequestInit) => Promise<unknown>>(routeService('one'));
        vi.stubGlobal('fetch', fetch);
        const body = (call: number) => JSON.parse(fetch.mock.calls[call][1].body as string);
        const cache = new LegCache();
        const signal = new AbortController().signal;
        const plan: Trip = { ...trip(), routeOrder: ['p1', 'p2', 'p3'], points: [7.8, 7.85, 7.9, 7.95, 8].map((lon, i) => ({ id: `p${i}`, kind: i ? i < 4 ? 'via' : 'finish' : 'start', coordinate: [lon, 48], label: '' })) };
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
    it('tells the rider why routing failed and passes a caller abort through', async () => {
        const fetch = vi.fn(async (_url: string, init: RequestInit): Promise<unknown> => { init.signal!.throwIfAborted(); throw new TypeError('Failed to fetch'); });
        vi.stubGlobal('fetch', fetch);
        const route = (signal = new AbortController().signal) => requestRoute([[7.8, 48], [8, 48]], 'touring', signal);
        await expect(route()).rejects.toThrow('The routing service is unreachable. Check your connection and retry.');
        fetch.mockResolvedValueOnce({ ok: false, status: 502, json: async () => JSON.parse('<html>') });
        await expect(route()).rejects.toThrow('Routing is unavailable. Retry shortly.');
        vi.spyOn(AbortSignal, 'timeout').mockReturnValueOnce(AbortSignal.abort(new DOMException('Timed out', 'TimeoutError')));
        await expect(route()).rejects.toThrow('The routing service did not answer in time. Retry shortly.');
        const abort = new AbortController();
        abort.abort();
        await expect(route(abort.signal)).rejects.toMatchObject({ name: 'AbortError' });
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
        const legs = (modes: RoutePoint['leg'][]): Trip => ({ ...trip(), points: trip().points.map((p, i) => ({ ...p, leg: modes[i] })) });
        const line = await calculateLine(legs([undefined, 'straight', 'straight']), new AbortController().signal, new LegCache());
        expect(line.unroutedKm).toBeCloseTo(line.unknownSurfaceKm);
        expect(line.elapsed.at(-1)).toBe(line.seconds);
        expect(line.elevation.every(h => h === null)).toBe(true);
        expect(line.edges).toEqual({});
        expect(surfaceRuns(line).shares.get('Unknown')).toBe(1);
        const mixed = await calculateLine(legs([undefined, 'straight', 'routed']), new AbortController().signal, new LegCache());
        expect(mixed.edges).toEqual({ surfaces: [null, null, 'Paved', 'Gravel'], pushing: [null, null, false, true] });
    });
    it('aligns surface sections by distance and clips the view without changing route shares', () => {
        const line = { coordinates: route.geometry, edges: route.edges };
        const data = surfaceRuns(line);
        expect(data.runs.map(run => run.surface)).toEqual(['Paved', 'Gravel']);
        expect(data.runs[0].to).toBeCloseTo(.5);
        expect(data.runs[1].from).toBe(data.runs[0].to);
        const clipped = surfaceWindow(data.runs, .25, .75);
        expect(clipped[0]).toEqual({ ...data.runs[0], from: .25 });
        expect(clipped[1]).toEqual({ ...data.runs[1], to: .75 });
        expect(data.shares.get('Gravel')).toBeCloseTo(.5);
        expect(surfaceRuns({ ...line, edges: {} }).shares.get('Unknown')).toBe(1);
        expect(surfaceRuns().runs).toEqual([]);
        const sameSurface = surfaceRuns({ ...line, edges: { ...line.edges, surfaces: ['Paved', 'Paved'] } });
        expect(sameSurface.runs.map(run => run.pushing)).toEqual([false, true]);
        expect(sameSurface.shares.get('Paved')).toBe(1);
    });
    it('reports service failure and never substitutes fixture geometry', async () => {
        vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: false, json: async () => ({ code: 'no_path', message: 'No legal route.' }) }));
        const plan = trip();
        await expect(calculateLine(plan, new AbortController().signal, new LegCache())).rejects.toThrow('No legal route.');
        expect(planView(plan, undefined).coordinates).toEqual([plan.points[0].coordinate]);
    });
});

describe('joins beside manual legs', () => {
    // Snaps every point about 5 m north and knows every height, as `/v1/route` does on a road beside the point.
    const snapping = async (_url: string, init: RequestInit) => {
        const { points } = JSON.parse(init.body as string) as { points: Coordinate[] };
        const snapped = points.map(([lon, lat]): Coordinate => [lon, lat + .000045]);
        const legTotals = { ...totals(1000), unknown_elevation_m: 0 };
        const routes: AnswerRoute[] = [{ id: 'snapped', reason: 'primary', package: 'p', profile: 'touring', snap_truncated: false,
            coordinates_udeg: snapped.flat().map((value, i, flat) => Math.round(value * 1e6) - (i > 1 ? Math.round(flat[i - 2] * 1e6) : 0)),
            elevation_dm: snapped.map(() => 2000), elapsed_s: snapped.map((_, i) => i && 1000), totals: { ...totals(1000 * (points.length - 1)), unknown_elevation_m: 0 },
            legs: snapped.slice(1).map((_, k) => ({ from_index: k, to_index: k + 1, start: `${k}`, end: `${k + 1}`, totals: legTotals })) }];
        return { ok: true, json: async () => ({ routes }) };
    };
    const point = (id: string, kind: RoutePoint['kind'], lon: number, extra: Partial<RoutePoint> = {}): RoutePoint =>
        ({ id, kind, label: id, coordinate: [lon, 48], ...extra });

    it('knows the heights of a routed day beside a transfer or a drawn line with heights', async () => {
        vi.stubGlobal('fetch', vi.fn(snapping));
        const signal = new AbortController().signal;
        const roads: Trip = { ...trip(), mode: 'trip', days: 2, routeOrder: ['night-1', 'via'], points: [point('start', 'start', 7.8),
            point('night-1', 'night', 7.85, { night: 1 }), point('via', 'via', 7.95, { leg: 'transfer' }), point('finish', 'finish', 8)] };
        const transferred = await calculateLine(roads, signal, new LegCache());
        expect(transferred).toMatchObject({ unknownElevationKm: 0, unroutedKm: 0 });
        const kept: Trip = { ...trip(), mode: 'trip', days: 2, routeOrder: ['night-1'], points: [point('start', 'start', 7.8),
            point('night-1', 'night', 7.85, { night: 1 }), point('finish', 'finish', 7.9, { leg: 'drawn', drawn: [[7.85, 48, 200], [7.88, 48, 210], [7.9, 48, 220]] })] };
        expect((await calculateLine(kept, signal, new LegCache())).unknownElevationKm).toBe(0);
    });
});

