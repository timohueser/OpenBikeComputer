import { afterEach, describe, expect, it, vi } from 'vitest';
import { cumulative, kilometres, orderedRoutePoints, riddenKm, routeCoordinates, tripDays, type Coordinate, type DrawnCoordinate } from './editor';
import { profileAscent } from './profile-data';
import { importedTrip, planOnRoads, readTracks } from './gpx-import';
import { LegCache } from './route-legs';
import { calculateLine, requestShape } from './routing';
import { isTrip } from './trip-validation';

const gpx = (line: DrawnCoordinate[], name?: string) => `<?xml version="1.0"?><gpx>${name ? `<trk><name>${name}</name>` : '<trk>'}<trkseg>${
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
        const ridden = cumulative(routeCoordinates(routed)).at(-1)! - kilometres(second.at(-1)!, third[0]);
        expect(riddenKm(routed)).toBeCloseTo(ridden, 9);
        expect(routed.routing.seconds).toBeCloseTo(ridden / 15 * 3600, 6);
        expect(routed.routing.unroutedKm).toBeCloseTo(ridden, 9);
        expect(routed.routing.unknownElevationKm).toBe(0);
        expect(profileAscent(0, 1, routed.routing)).toBe(57);
        const days = tripDays(routed);
        expect(days.reduce((km, day) => km + day.distance, 0)).toBeCloseTo(ridden, 9);
        expect(days[2].hours).toBeCloseTo(cumulative(third).at(-1)! / 15, 6);
    });

    it('rejects more files than a trip has days', () => {
        const files = Array.from({ length: 15 }, (_, i) => ({ name: `${i}.gpx`, text: gpx(track([7.6, 47.5], 2)) }));
        expect(() => readTracks(files)).toThrow('A trip has at most 14 days. Import 14 files or fewer.');
    });

    it('routes a shaped file through its points and keeps the line of a file that cannot be shaped', async () => {
        const fetch = vi.fn()
            .mockResolvedValueOnce({ ok: true, json: async () => ({ points: [[7.6, 47.5], [7.62, 47.51], [7.65, 47.5]], turnarounds: [1] }) })
            .mockResolvedValueOnce({ ok: false, json: async () => ({ code: 'line_not_reproducible', message: 'The line cannot be reproduced.' }) });
        vi.stubGlobal('fetch', fetch);
        const lines = readTracks([{ name: 'a.gpx', text: gpx(track([7.6, 47.5], 50)) }, { name: 'b.gpx', text: gpx(track([7.65, 47.5], 50)) }]);
        const planned = await planOnRoads(lines, line => requestShape(line, 'touring'));
        expect(JSON.parse(fetch.mock.calls[0][1].body)).toEqual({ line: lines[0].line, profile: 'touring' });
        expect(planned.failed).toEqual(['b']);
        const trip = importedTrip({}, planned.lines);
        expect(isTrip(trip)).toBe(true);
        expect(orderedRoutePoints(trip).map(p => [p.kind, p.leg, p.turnaround])).toEqual([
            ['start', undefined, undefined], ['via', undefined, true], ['night', undefined, undefined], ['finish', 'drawn', undefined]]);
    });
});
