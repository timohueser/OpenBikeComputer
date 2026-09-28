import { afterEach, describe, expect, it, vi } from 'vitest';
import { calculateLine, selectRoute, movingSecondsAt, type EngineRoute } from './routing';
import { surfaceRuns, surfaceWindow } from './surface-data';
import { initialTrip, cumulative, routingKey, routeCoordinates, type Trip } from './editor';

const route: EngineRoute = {
    id: 'test-route', surfaces: ['Paved', 'Gravel'], reason: 'primary', elapsed: [0, 3000, 4000], package: 'test', profile: 'touring', geometry: [[7.8, 48], [7.9, 48], [8, 48]], elevation: [200, null, 400],
    totals: { distance_m: 15000, ascent_m: 0, descent_m: 0, seconds: 4000, surface_m: [1000, 14000, 0, 0, 0, 0], unknown_elevation_m: 15000, pushing_m: 100 },
    legs: [{ from_index: 0, to_index: 1, totals: { distance_m: 7500 } as EngineRoute['totals'] }, { from_index: 1, to_index: 2, totals: { distance_m: 7500 } as EngineRoute['totals'] }],
    snap_truncated: false,
};
function trip(): Trip {
    return { ...initialTrip(), live: true, points: [
        { id: 'start', kind: 'start', coordinate: [7.8, 48], label: 'Start', progress: 0 },
        { id: 'shape', kind: 'via', coordinate: [7.9, 48], label: 'Shape', progress: .5 },
        { id: 'finish', kind: 'finish', coordinate: [8, 48], label: 'End', progress: 1 },
    ] };
}
afterEach(() => vi.unstubAllGlobals());
describe('routing integration', () => {
    it('keeps directed shaping context in one request and preserves unknown elevation', async () => {
        const fetch = vi.fn().mockResolvedValue({ ok: true, json: async () => ({ routes: [route] }) });
        vi.stubGlobal('fetch', fetch);
        const plan = trip();
        const line = await calculateLine(plan, new AbortController().signal);
        expect(fetch).toHaveBeenCalledTimes(1);
        expect(JSON.parse(fetch.mock.calls[0][1].body).alternatives).toBe(false);
        expect(line.alternativesReady).toBe(false);
        expect(line.surfaces).toEqual(['Paved', 'Gravel']);
        const withAlternatives = await calculateLine(plan, new AbortController().signal, true);
        expect(JSON.parse(fetch.mock.calls[1][1].body).alternatives).toBe(true);
        expect(withAlternatives.alternativesReady).toBe(true);
        expect(JSON.parse(fetch.mock.calls[0][1].body).points).toEqual(plan.points.map(p => p.coordinate));
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
        const moved = { ...edited, points: edited.points.map(p => p.id === 'shape' ? { ...p, coordinate: [8.1, 48] as [number, number] } : p) };
        expect(routingKey(moved)).not.toBe(routingKey(plan));
        expect(routeCoordinates(moved)).toEqual([plan.points[0].coordinate]);
    });
    it('makes visit reversals explicit and accounts for manual joins', async () => {
        const fetch = vi.fn().mockResolvedValue({ ok: true, json: async () => ({ routes: [{ ...route, legs: Array.from({ length: 4 }, (_, i) => ({ from_index: 0, to_index: Math.min(i, 2), totals: route.totals })) }] }) });
        vi.stubGlobal('fetch', fetch);
        const plan = trip();
        plan.points[1] = { ...plan.points[1], kind: 'detour', anchor: [7.85, 48] };
        await calculateLine(plan, new AbortController().signal);
        const body = JSON.parse(fetch.mock.calls[0][1].body);
        expect(body.points).toEqual([[7.8, 48], [7.85, 48], [7.9, 48], [7.85, 48], [8, 48]]);
        expect(body.turnarounds).toEqual([2]);
        expect(body.alternatives).toBe(false);
        const manual = trip();
        manual.points[1].leg = 'straight';
        manual.points[2].leg = 'straight';
        const line = await calculateLine(manual, new AbortController().signal);
        expect(line.unroutedKm).toBeCloseTo(line.unknownSurfaceKm);
        expect(line.elapsed.at(-1)).toBe(line.seconds);
        expect(line.elevation.every(h => h === null)).toBe(true);
        expect(line.surfaces).toHaveLength(line.coordinates.length - 1);
        expect(line.surfaces.every(s => s === 'Unknown')).toBe(true);
        manual.points[2].leg = 'routed';
        const mixed = await calculateLine(manual, new AbortController().signal);
        expect(mixed.surfaces).toEqual(['Unknown', 'Unknown', 'Paved', 'Gravel']);
        expect(mixed.surfaces).toHaveLength(mixed.coordinates.length - 1);
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
    });
    it('reports service failure and never substitutes fixture geometry', async () => {
        vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: false, json: async () => ({ code: 'no_path', message: 'No legal route.' }) }));
        const plan = trip();
        await expect(calculateLine(plan, new AbortController().signal)).rejects.toThrow('No legal route.');
        expect(routeCoordinates(plan)).toEqual([plan.points[0].coordinate]);
    });
});
