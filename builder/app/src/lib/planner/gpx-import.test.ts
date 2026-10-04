import { afterEach, describe, expect, it, vi } from 'vitest';
import { applyBudget, orderedRoutePoints, planView, type DrawnCoordinate } from './editor';
import { cumulative, kilometres, type Coordinate } from './geo';
import { profileAscent } from './profile-data';
import { importedTrip, planOnRoads, readTracks } from './gpx-import';
import { exportPlan, importPlan, newPlan } from './library';
import { LegCache } from './route-legs';
import { calculateLine, requestShape } from './routing';
import { isTrip } from './trip-validation';

const gpx = (line: DrawnCoordinate[], name?: string, wpts = '') => `<?xml version="1.0"?><gpx>${wpts}${name ? `<trk><name>${name}</name>` : '<trk>'}<trkseg>${
    line.map(([lon, lat, ele]) => `<trkpt lat="${lat}" lon="${lon}">${ele === undefined ? '' : `<ele>${ele}</ele>`}</trkpt>`).join('')}</trkseg></trk></gpx>`;
// Points every 0.001° of longitude, about 75 m at this latitude.
const track = (from: Coordinate, count: number): Coordinate[] => Array.from({ length: count }, (_, i) => [from[0] + i * .001, from[1]]);

afterEach(() => vi.unstubAllGlobals());

describe('GPX import', () => {
    it('keeps one file as a route with one drawn leg, without points that do not change the line', async () => {
        const line = track([7.6, 47.5], 50);
        const noisy = line.map(([lon, lat], i): Coordinate => [lon, lat + (i === 25 ? .001 : i % 2 ? 1e-6 : 0)]);
        const [file] = readTracks([{ name: 'ride.gpx', text: gpx(noisy) }]);
        expect(file.name).toBe('ride');
        const micro = (line: DrawnCoordinate[]) => line.map(p => p.map(n => Math.round(n * 1e6)));
        expect(micro(file.line)).toEqual(micro([noisy[0], noisy[24], noisy[25], noisy[26], noisy[49]]));
        const trip = importedTrip({ bike: 'gravel' }, [file]);
        expect(isTrip(trip)).toBe(true);
        expect(trip).toMatchObject({ mode: 'route', bike: 'gravel', live: true });
        expect(orderedRoutePoints(trip).map(p => [p.kind, p.leg])).toEqual([['start', undefined], ['finish', 'drawn']]);
        expect(trip.points[1].drawn).toEqual(file.line);
        expect((await calculateLine(trip, new AbortController().signal, new LegCache())).unknownElevationKm).toBeGreaterThan(0);
        expect(readTracks([{ name: 'a.gpx', text: gpx(line, 'Jura crossing') }])[0].name).toBe('Jura crossing');
    });

    it('makes a trip with a night at each day end and a transfer that is not ridden or climbed', async () => {
        const first = track([7.6, 47.5], 20), second = track([first.at(-1)![0] + .001, 47.5], 20), third = track([7.8, 47.6], 20);
        // Each day climbs 19 m; day 3 starts 362 m higher, across the transfer.
        const heights = [100, 119, 500].map((base, day) => [first, second, third][day].map(([lon, lat], i): DrawnCoordinate => [lon, lat, base + i]));
        const lines = readTracks(heights.map((line, i) => ({ name: `day-${i + 1}.gpx`, text: gpx(line) })));
        const trip = importedTrip({}, lines);
        expect(isTrip(trip)).toBe(true);
        expect(trip).toMatchObject({ mode: 'trip', days: 3, target: 3 });
        expect(orderedRoutePoints(trip).map(p => [p.id.startsWith('night') ? p.id : p.kind, p.leg]))
            .toEqual([['start', undefined], ['night-1', 'drawn'], ['night-2', 'drawn'], ['via', 'transfer'], ['finish', 'drawn']]);
        const routed = { ...trip, routing: await calculateLine(trip, new AbortController().signal, new LegCache()) };
        const ridden = planView(routed).total - kilometres(second.at(-1)!, third[0]);
        expect(planView(routed).summary.distance).toBeCloseTo(ridden, 9);
        expect(routed.routing.seconds).toBeCloseTo(ridden / 19 * 3600, 6);
        expect(routed.routing.unroutedKm).toBeCloseTo(ridden, 9);
        expect(routed.routing.unknownElevationKm).toBe(0);
        expect(profileAscent(0, 1, routed.routing)).toBe(57);
        const days = planView(routed).days;
        expect(days.reduce((km, day) => km + day.distance, 0)).toBeCloseTo(ridden, 9);
        expect(days[2].hours).toBeCloseTo(cumulative(third).at(-1)! / 19, 6);
    });

    it('budgets the hours of a kept line at the pace of its activity', async () => {
        // About 60 km: 13.4 h of walking at 4.5 km/h, or 3.2 h of riding at 19 km/h.
        const [file] = readTracks([{ name: 'walk.gpx', text: gpx(track([7.6, 47.5], 801)) }]);
        const days = async (bike: 'hiking' | 'gravel') => {
            const trip = importedTrip({ bike }, [file]);
            const routed = { ...trip, routing: await calculateLine(trip, new AbortController().signal, new LegCache()) };
            return applyBudget(routed, 'hours', 6, 0).days;
        };
        expect([await days('hiking'), await days('gravel')]).toEqual([3, 1]);
    });

    it('keeps file waypoints as markers that survive save and reload', () => {
        const wpts = '<wpt lat="47.501" lon="7.61"><name>Spring &amp; bench</name><cmt>Tap</cmt><desc> Cold water </desc><sym>Drinking Water</sym></wpt><wpt lat="47.499" lon="7.62"><desc> </desc></wpt><wpt lat="x" lon="7.6"/>';
        const lines = readTracks([{ name: 'ride.gpx', text: gpx(track([7.6, 47.5], 50), undefined, wpts) }]);
        expect(lines[0].name).toBe('ride');
        const trip = importedTrip({}, lines);
        expect(orderedRoutePoints(trip).map(p => [p.kind, p.leg])).toEqual([['start', undefined], ['finish', 'drawn']]);
        const markers = [{ kind: 'marker', label: 'Spring & bench', coordinate: [7.61, 47.501], note: 'Cold water' }, { kind: 'marker', label: 'Marker', coordinate: [7.62, 47.499] }];
        expect(trip.points.filter(p => p.kind === 'marker')).toMatchObject(markers);
        expect(trip.points.at(-1)).not.toHaveProperty('note');
        expect(importPlan(exportPlan(newPlan(trip, 'ride'))).trip.points.filter(p => p.kind === 'marker')).toMatchObject(markers);
    });

    it('keeps a valid plan when a file with waypoints has a zero-length line', () => {
        const trip = importedTrip({}, readTracks([{ name: 'still.gpx', text: gpx([[7.6, 47.5], [7.6, 47.5]], undefined, '<wpt lat="47.51" lon="7.6"/>') }]));
        expect(isTrip(trip)).toBe(true);
        expect(trip.points.find(p => p.kind === 'marker')!.progress).toBe(.5);
    });

    it('rejects more files than a trip has days', () => {
        const files = Array.from({ length: 15 }, (_, i) => ({ name: `${i}.gpx`, text: gpx(track([7.6, 47.5], 2)) }));
        expect(() => readTracks(files)).toThrow('A trip has at most 14 days. Import 14 files or fewer.');
    });

    it('routes a shaped file through its points and keeps the line of a file that cannot be shaped', async () => {
        const fetch = vi.fn()
            .mockResolvedValueOnce({ ok: true, json: async () => ({ points: [[7.6, 47.5], [7.62, 47.51], [7.65, 47.5]], turnarounds: [1] }) })
            .mockResolvedValueOnce({ ok: false, status: 503, json: async () => ({ code: 'busy', message: 'The routing service is busy.' }) });
        vi.stubGlobal('fetch', fetch);
        const lines = readTracks([
            { name: 'a.gpx', text: gpx(track([7.6, 47.5], 50), undefined, '<wpt lat="47.51" lon="7.62"><name>Hut</name></wpt>') },
            { name: 'b.gpx', text: gpx(track([7.65, 47.5], 50), undefined, '<wpt lat="47.49" lon="7.67"><name>Cafe</name></wpt><wpt lat="47.5" lon="7.65"><name>Gate</name></wpt>') }]);
        const planned = await planOnRoads(lines, line => requestShape(line, 'touring'));
        expect(JSON.parse(fetch.mock.calls[0][1].body)).toEqual({ line: lines[0].line, profile: 'touring' });
        expect(planned.failed).toEqual(['b']);
        const trip = importedTrip({}, planned.lines);
        expect(isTrip(trip)).toBe(true);
        expect(orderedRoutePoints(trip).map(p => [p.kind, p.leg, p.turnaround])).toEqual([
            ['start', undefined, undefined], ['via', undefined, true], ['night', undefined, undefined], ['finish', 'drawn', undefined]]);
        expect(trip.points.filter(p => p.kind === 'marker').map(p => p.label)).toEqual(['Hut', 'Cafe', 'Gate']);
        // A waypoint at the first point of file 2 starts day 2.
        expect(trip.points.find(p => p.label === 'Gate')!.progress).toBe(1 / 2);
        expect(trip.routeOrder).toHaveLength(2);
    });
});
