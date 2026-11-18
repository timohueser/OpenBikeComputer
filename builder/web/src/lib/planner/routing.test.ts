import { afterEach, describe, expect, it, vi } from 'vitest';
import { calculateLine, pacedLine, profileId, regionProfiles, requestAlternatives, requestRoute, routingKey, movingSecondsAt, type RouteTotals, type RoutingLine, type Surface } from './routing';
import { chunks, LegCache } from './route-legs';
import { routeService } from '../../../test-support/planner/route-service';
import { decodeRoutes, type AnswerRoute } from './route-answer';
import { paceFactor, presetName, servedPresets, type BikeType } from './riding-profiles';
import { surfaceRuns, surfaceShares, surfaceWindow } from './surface-data';
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
        const profiles = ['gravel', 'gravel/less-climbing', 'hiking/alpine', 'mtb/technical'];
        vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: true, json: async () => ({ package: 'test', profiles }) }));
        const served = await regionProfiles();
        expect([servedPresets('gravel', served), servedPresets('hiking', served), servedPresets('mtb', served), servedPresets('road', served), servedPresets('road', undefined), servedPresets('mtb', undefined)])
            .toEqual([['Balanced', 'Less climbing'], ['Alpine'], ['Technical'], [], ['Balanced', 'Less climbing'], ['Balanced']]);
        for (const id of profiles) expect(profileId({ ...trip(), bike: id.split('/')[0] as BikeType, preset: presetName(id) })).toBe(id);
        // A saved plan with a preset that its activity does not have routes with Balanced.
        expect(profileId({ ...trip(), bike: 'road', preset: 'Shorter' })).toBe('road');
        expect(profileId({ ...trip(), bike: 'mtb', preset: 'Less climbing' })).toBe('mtb');
        expect(routingKey({ ...trip(), bike: 'mtb', preset: 'Less climbing' })).toBe(routingKey({ ...trip(), bike: 'mtb' }));
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
        fetch.mockResolvedValueOnce({ ok: true, json: async () => ({ routes: [{ ...answer, id: 'climb', reason: 'less_climbing' }] }) });
        const alternatives = await requestAlternatives(plan, line.primary!, new AbortController().signal);
        const { alternatives: _primaryOnly, ...request } = JSON.parse(fetch.mock.calls[0][1].body);
        expect(JSON.parse(fetch.mock.calls[1][1].body)).toEqual({ ...request, alternatives_only: true });
        expect(alternatives.map(r => r.id)).toEqual(['primary', 'climb']);
        expect(movingSecondsAt(line, 0.5)).toBeCloseTo(3000);
        expect(line.elevation).toEqual([200, null, 400]);
        expect(line.stops.map(s => s.distance)).toEqual(cumulative(route.geometry));
        expect(planView(plan, line).coordinates).toEqual(route.geometry);
    });
    it('keeps itinerary edits, labels and markers outside the routing key', () => {
        const plan = trip();
        const edited = { ...plan, days: 5, splits: { 1: .4 }, restAfter: [1], daysSet: true as const, points: plan.points.map(p => ({ ...p, label: 'Renamed' })) };
        expect(routingKey(edited)).toBe(routingKey(plan));
        expect(routingKey(setEndpoint(edited, 'start', edited.points[0].coordinate, 'New label'))).toBe(routingKey(plan));
        const marked = { ...edited, points: [...edited.points, { ...edited.points[0], id: 'note', kind: 'marker' as const }] };
        const unmarked = removeRoutePoint(marked, 'note');
        expect([routingKey(marked), routingKey(unmarked)]).toEqual([routingKey(plan), routingKey(plan)]);
        expect(unmarked.splits).toEqual(edited.splits);
        const moved = { ...edited, points: edited.points.map(p => p.id === 'shape' ? { ...p, coordinate: [8.1, 48] as [number, number] } : p) };
        expect(routingKey(moved)).not.toBe(routingKey(plan));
        expect(routingKey({ ...plan, preset: 'Less climbing' })).not.toBe(routingKey(plan));
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
        fetch.mockImplementationOnce(async () => ({ ok: false, json: async () => ({ code: 'limit', message: 'Too long.' }) }));
        await expect(calculateLine(move(7.94), signal, cache)).rejects.toThrow('Too long.');
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
        expect(surfaceShares(surfaceRuns(line))).toEqual([{ surface: 'Unknown', percent: 100 }]);
        const mixed = await calculateLine(legs([undefined, 'straight', 'routed']), new AbortController().signal, new LegCache());
        expect(mixed.edges).toEqual({ surfaces: [null, null, 'Paved', 'Gravel'], pushing: [null, null, false, true] });
    });
    it('times each activity at its own speed on the flat, and each pace against Moderate', () => {
        const flat = (bike: BikeType, pace?: Trip['pace']) => Math.round(({ road: 19, gravel: 19, touring: 19, mtb: 15.6, hiking: 4.5 })[bike] / paceFactor({ bike, pace }) * 10) / 10;
        expect((['road', 'gravel', 'touring', 'mtb', 'hiking'] as const).map(bike => (['easy', undefined, 'sporty', 'fast'] as const).map(pace => flat(bike, pace))))
            .toEqual([[19.7, 25, 30.3, 35.5], [16.6, 21, 25.4, 29.8], [15, 19, 23, 27], [12.3, 15.6, 18.9, 22.2], [3.6, 4.5, 5.4, 6.4]]);
    });
    it('scales the riding time of a routed or drawn segment at the pace of the plan, but not of a pushed segment', async () => {
        vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: true, json: async () => ({ routes: [{ ...answer, legs: [{ ...answer.legs[0], to_index: 2 }] }] }) }));
        const plan: Trip = { ...trip(), points: trip().points.map(p => p.id === 'shape' ? { ...p, leg: 'straight' } : p) };
        const line = await calculateLine(plan, new AbortController().signal, new LegCache());
        const fast: Trip = { ...plan, pace: 'fast' }, paced = pacedLine(line, fast);
        const time = (l: RoutingLine, i: number) => l.elapsed[i + 1] - l.elapsed[i];
        expect(line.edges.pushing).toEqual([null, null, false, true]);
        expect(time(line, 0)).toBeGreaterThan(0);
        line.edges.pushing!.forEach((pushed, i) => expect(time(paced, i)).toBeCloseTo(time(line, i) * (pushed ? 1 : 19 / 27)));
        expect(paced.seconds).toBe(paced.elapsed.at(-1));
        const lines = [pacedLine(line, fast), pacedLine(paced, fast), pacedLine(paced, plan), pacedLine(line, { ...plan, bike: 'mtb' })];
        expect(lines.map(l => l === paced ? 'paced' : l === line ? 'line' : 'copy')).toEqual(['paced', 'paced', 'line', 'line']);
        // A road bike at Moderate rides faster than the engine; on foot, every stretch takes the pace.
        const road = pacedLine(line, { ...plan, bike: 'road' });
        line.edges.pushing!.forEach((pushed, i) => expect(time(road, i)).toBeCloseTo(time(line, i) * (pushed ? 1 : 19 / 25)));
        expect(pacedLine(line, { ...plan, bike: 'hiking', pace: 'easy' }).seconds).toBeCloseTo(line.seconds * 19 / 15);
        expect(pacedLine(line, { ...plan, bike: 'hiking', pace: 'fast' }).seconds).toBeCloseTo(line.seconds * 19 / 27);
    });
    it('aligns surface sections by distance and clips the view without changing route shares', () => {
        const line = { coordinates: route.geometry, edges: route.edges };
        const runs = surfaceRuns(line);
        expect(runs.map(run => run.surface)).toEqual(['Paved', 'Gravel']);
        expect(runs[0].to).toBeCloseTo(.5);
        expect(runs[1].from).toBe(runs[0].to);
        const clipped = surfaceWindow(runs, .25, .75);
        expect(clipped[0]).toEqual({ ...runs[0], from: .25 });
        expect(clipped[1]).toEqual({ ...runs[1], to: .75 });
        expect(surfaceShares(runs)).toEqual([{ surface: 'Paved', percent: 50 }, { surface: 'Gravel', percent: 50 }]);
        expect(surfaceRuns({ ...line, edges: {} }).map(run => run.surface)).toEqual(['Unknown']);
        expect(surfaceRuns()).toEqual([]);
        const sameSurface = surfaceRuns({ ...line, edges: { ...line.edges, surfaces: ['Paved', 'Paved'] } });
        expect(sameSurface.map(run => run.pushing)).toEqual([false, true]);
        expect(sameSurface.map(run => run.surface)).toEqual(['Paved', 'Paved']);
    });
    it('gives the surface shares in whole percent that add to 100, the largest first and Unknown last', () => {
        const shares = (lengths: [Surface, number][]) => {
            let at = 0;
            return surfaceShares(lengths.map(([surface, length]) => ({ surface, from: at, to: at += length })));
        };
        // 62.3, 25.3 and 12.4 percent round down to 99: the largest fraction takes the point that remains.
        expect(shares([['Gravel', .124], ['Paved', .4], ['Dirt', .253], ['Paved', .223]]))
            .toEqual([{ surface: 'Paved', percent: 62 }, { surface: 'Dirt', percent: 25 }, { surface: 'Gravel', percent: 13 }]);
        // The shares are those of the runs, which can be a part of the route.
        expect(shares([['Paved', .1], ['Gravel', .3]])).toEqual([{ surface: 'Gravel', percent: 75 }, { surface: 'Paved', percent: 25 }]);
        expect(shares([['Paved', .996], ['Rough', .004]])).toEqual([{ surface: 'Paved', percent: 100 }, { surface: 'Rough', percent: 0 }]);
        expect(shares([['Unknown', .5], ['Dirt', .2], ['Paved', .3]]))
            .toEqual([{ surface: 'Paved', percent: 30 }, { surface: 'Dirt', percent: 20 }, { surface: 'Unknown', percent: 50 }]);
        expect(shares([])).toEqual([]);
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


describe('runs longer than one request', () => {
    const signal = new AbortController().signal;
    const run = (count: number) => Array.from({ length: count }, (_, i): Coordinate => [8 + i / 1000, 48]);
    const bodies = (fetch: { mock: { calls: unknown[][] } }) => fetch.mock.calls.map(call => JSON.parse((call[1] as RequestInit).body as string));
    const failing = (code: string, message: string) => async () => ({ ok: false, json: async () => ({ code, message }) });
    afterEach(() => vi.useRealTimers());

    it('cuts the legs into requests that share their ends, with no end at a turnaround', () => {
        expect(chunks(0, 228, [])).toEqual([[0, 62], [63, 125], [126, 188], [189, 228]]);
        expect(chunks(5, 70, [68])).toEqual([[5, 66], [67, 70]]);
    });

    it('sends the chunks one after the other, each pinned to the end of the chunk before', async () => {
        let open = 0, most = 0;
        const service = routeService('one');
        const fetch = vi.fn(async (url: string, init: RequestInit) => {
            most = Math.max(most, ++open);
            await new Promise(resolve => setTimeout(resolve));
            open--;
            return service(url, init);
        });
        vi.stubGlobal('fetch', fetch);
        const cache = new LegCache();
        const points = run(230);
        const route = await cache.route(points, [], 'touring', signal);
        const sent = bodies(fetch);
        expect(sent.map(body => body.points.length)).toEqual([64, 64, 64, 41]);
        sent.slice(1).forEach((body, c) => expect([body.points[0], body.start_position]).toEqual([sent[c].points.at(-1), `one:${sent[c].points.at(-1)}`]));
        expect([most, route.legs.length]).toEqual([1, 229]);
        expect(route.geometry).toEqual(points);
        await cache.route(points, [], 'touring', signal);
        expect(fetch).toHaveBeenCalledTimes(4);
    });

    it('asks for each gap of an edit by itself, pinned to its own cached neighbours', async () => {
        const fetch = vi.fn<(url: string, init: RequestInit) => Promise<unknown>>(routeService('one'));
        vi.stubGlobal('fetch', fetch);
        const cache = new LegCache();
        const points = run(10);
        await cache.route(points, [], 'touring', signal);
        const edited = points.map((point, i): Coordinate => i === 2 || i === 7 ? [point[0], 48.01] : point);
        expect((await cache.route(edited, [], 'touring', signal)).geometry).toEqual(edited);
        expect(bodies(fetch).slice(1)).toMatchObject([
            { points: edited.slice(1, 4), start_position: `one:${points[1]}`, end_position: `one:${points[3]}` },
            { points: edited.slice(6, 9), start_position: `one:${points[6]}`, end_position: `one:${points[8]}` }]);
        expect(fetch).toHaveBeenCalledTimes(3);
    });

    it('sends a gap without a path again with one more leg on each side, not the whole run', async () => {
        const fetch = vi.fn<(url: string, init: RequestInit) => Promise<unknown>>(routeService('one'));
        vi.stubGlobal('fetch', fetch);
        const cache = new LegCache();
        const points = run(230);
        await cache.route(points, [], 'touring', signal);
        const dragged = points.map((point, i): Coordinate => i === 115 ? [point[0], 48.01] : point);
        fetch.mockImplementationOnce(failing('no_path', 'No legal route.'));
        expect((await cache.route(dragged, [], 'touring', signal)).geometry).toEqual(dragged);
        expect(bodies(fetch).slice(4)).toMatchObject([
            { points: dragged.slice(114, 117), start_position: `one:${points[114]}`, end_position: `one:${points[116]}` },
            { points: dragged.slice(113, 118), start_position: `one:${points[113]}`, end_position: `one:${points[117]}` }]);
        expect(fetch).toHaveBeenCalledTimes(6);
    });

    it('sends a chunk without a path under the pin of the chunk before again from one leg earlier', async () => {
        const fetch = vi.fn<(url: string, init: RequestInit) => Promise<unknown>>(routeService('one'));
        vi.stubGlobal('fetch', fetch);
        fetch.mockImplementationOnce(routeService('one')).mockImplementationOnce(failing('no_path', 'No legal route.'));
        const points = run(100);
        expect((await new LegCache().route(points, [], 'touring', signal)).geometry).toEqual(points);
        expect(bodies(fetch).slice(1)).toMatchObject([
            { points: points.slice(63), start_position: `one:${points[63]}` }, { points: points.slice(62), start_position: `one:${points[62]}` }]);
        expect(fetch).toHaveBeenCalledTimes(3);
    });

    it('fails a run without cached legs after the widened request, not after the whole run again', async () => {
        const points = run(230);
        const fetch = vi.fn(async (url: string, init: RequestInit) => JSON.parse(init.body as string).points.some(([lon]: Coordinate) => lon === points[150][0])
            ? failing('no_path', 'No legal route.')() : routeService('one')(url, init));
        vi.stubGlobal('fetch', fetch);
        await expect(new LegCache().route(points, [], 'touring', signal)).rejects.toThrow('No legal route.');
        expect(bodies(fetch).map(body => body.points[0][0])).toEqual([points[0][0], points[63][0], points[126][0], points[125][0]]);
    });

    it('keeps consecutive turnarounds inside the request of an edit', async () => {
        const fetch = vi.fn<(url: string, init: RequestInit) => Promise<unknown>>(routeService('one'));
        vi.stubGlobal('fetch', fetch);
        const cache = new LegCache();
        const points = run(8);
        await cache.route(points, [2, 3], 'touring', signal);
        const edited = points.map((point, i): Coordinate => i === 4 ? [point[0], 48.01] : point);
        await cache.route(edited, [2, 3], 'touring', signal);
        expect(bodies(fetch)[1]).toMatchObject({ points: edited.slice(1, 6), turnarounds: [1, 2], start_position: `one:${points[1]}` });
    });

    it('keeps each chunk as it arrives; a failure or an abort stops the chunks after it', async () => {
        const fetch = vi.fn<(url: string, init: RequestInit) => Promise<unknown>>(routeService('one'));
        vi.stubGlobal('fetch', fetch);
        fetch.mockImplementationOnce(routeService('one')).mockImplementationOnce(routeService('one')).mockImplementationOnce(failing('limit', 'Too long.'));
        const cache = new LegCache();
        const points = run(230);
        await expect(cache.route(points, [], 'touring', signal)).rejects.toThrow('Too long.');
        expect(fetch).toHaveBeenCalledTimes(3);
        expect((await cache.route(points, [], 'touring', signal)).geometry).toEqual(points);
        expect(bodies(fetch).slice(3)).toMatchObject([{ points: points.slice(126, 190), start_position: `one:${points[126]}` }, { points: points.slice(189) }]);
        const abort = new AbortController();
        fetch.mockImplementation(async (url, init) => { abort.abort(); return routeService('one')(url, init); });
        await expect(new LegCache().route(points, [], 'touring', abort.signal)).rejects.toThrow();
        expect(fetch).toHaveBeenCalledTimes(6);
    });

    it('sends a busy chunk again after a second, at most twice', async () => {
        vi.useFakeTimers({ toFake: ['setTimeout'] });
        const busy = failing('busy', 'Busy.');
        const fetch = vi.fn<(url: string, init: RequestInit) => Promise<unknown>>(routeService('one')).mockImplementationOnce(busy);
        vi.stubGlobal('fetch', fetch);
        const points = run(3);
        const route = new LegCache().route(points, [], 'touring', signal);
        await vi.advanceTimersByTimeAsync(1000);
        expect((await route).geometry).toEqual(points);
        fetch.mockImplementation(busy);
        const failed = expect(new LegCache().route(points, [], 'touring', signal)).rejects.toThrow('Busy.');
        await vi.advanceTimersByTimeAsync(2000);
        await failed;
        expect(fetch).toHaveBeenCalledTimes(5);
    });

    it('offers route options only for a plan that fits one request', async () => {
        vi.stubGlobal('fetch', vi.fn(routeService('one')));
        const plan = (count: number): Trip => {
            const points = run(count).map((coordinate, i): RoutePoint => ({ id: `p${i}`, kind: i ? i < count - 1 ? 'via' : 'finish' : 'start', coordinate, label: '' }));
            return { ...trip(), points, routeOrder: points.slice(1, -1).map(point => point.id) };
        };
        expect((await calculateLine(plan(64), signal, new LegCache())).primary).toBeDefined();
        expect((await calculateLine(plan(65), signal, new LegCache())).primary).toBeUndefined();
    });
});
