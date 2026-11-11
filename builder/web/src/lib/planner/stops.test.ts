import { describe, expect, it } from 'vitest';
import { groupStops, stopWindow, daysAround, evenStop, proposeNights, proposalTitle, stopCounts, stopKm, suitable } from './stops';
import { allStay, type Stay, type Trip } from './editor';
import type { SearchPlace } from './search/types';
import { testLine, testTrip } from '../../../test-support/planner/trip';

const place = (name: string, kind: string, city: string, along: number, off = .1): SearchPlace => ({
    source: `${kind}-${name}`, name, kind, city, region: '', lon: 8, lat: 48, distance: 0, precision: 'place', position: { along, distance: off },
});

describe('stops', () => {
    it('groups places to sleep by their gaps along the route, keeps a town together, and names each stop after its town', () => {
        const stops = groupStops([
            place('Gasthaus Ahorn', 'guest_house', 'Titisee-Neustadt', 38.4), place('Sonnenmatte', 'hotel', 'Titisee-Neustadt', 36.5),
            place('Charlott', 'hotel', 'Eisenbach', 40.8, .8), place('Feldbergblick', 'guest_house', 'Titisee-Neustadt', 45.5),
            place('Lindenhof', 'hotel', 'Bräunlingen', 51), place('Riedsee-Camping', 'campsite', 'Donaueschingen', 64.2, .25),
        ], [
            place('Schmidts Markt', 'supermarket', 'Titisee-Neustadt', 36.9), place('Löwen', 'restaurant', 'Titisee-Neustadt', 35.7),
            place('Far bakery', 'bakery', 'Titisee-Neustadt', 37, 2.4), place('Restaurant am Riedsee', 'restaurant', 'Donaueschingen', 64.1),
        ]);
        expect(stops.map(s => [s.name, s.from, s.to, s.sleep.map(p => p.place.label)])).toEqual([
            ['Titisee-Neustadt', 36.5, 45.5, ['Sonnenmatte', 'Gasthaus Ahorn', 'Feldbergblick', 'Charlott']],
            ['Bräunlingen', 51, 51, ['Lindenhof']],
            ['Donaueschingen', 64.2, 64.2, ['Riedsee-Camping']],
        ]);
        const valley = groupStops(['A', 'B', 'C', 'D', 'E'].map((name, i) => place(name, 'guest_house', name, 10 + i * 2)), []);
        expect(valley.map(s => s.name)).toEqual(['A', 'E']);
        expect(stops[0].shop.map(p => p.place.label)).toEqual(['Schmidts Markt']);
        expect(stops[0].food.map(p => p.place.label)).toEqual(['Löwen']);
        expect(stops[2].food.map(p => p.place.label)).toEqual(['Restaurant am Riedsee']);
    });

    it('counts and places a stop by the places of the kinds of a night only', () => {
        const [stop] = groupStops([
            place('Rigi', 'hotel', 'Arth', 42.4), place('Alp', 'hut', 'Arth', 43), place('Seecamping', 'campsite', 'Arth', 44.2),
            place('Adler', 'guest_house', 'Arth', 45.6), place('Waldhütte', 'shelter', 'Arth', 46),
        ], []);
        expect(suitable(stop, ['camp']).map(p => p.place.label)).toEqual(['Seecamping']);
        expect(suitable(stop, ['hostel'])).toEqual([]);
        expect(stopCounts(stop, allStay)).toBe('1 campsite · 1 hut · 2 hotels · 1 shelter');
        expect(stopCounts(stop, ['camp', 'shelter'])).toBe('1 campsite · 1 shelter');
        expect(stopCounts(stop, allStay, true)).toBe('5 places to sleep');
        expect(stopCounts(stop, ['camp', 'shelter'], true)).toBe('1 campsite · 1 shelter');
        expect(stopKm(stop, allStay)).toBe('km 42–46');
        expect(stopKm(stop, ['camp'])).toBe('km 44');
    });

    it('offers stops half a day either side of the even day end, with both days around each', () => {
        const trip = { ...testTrip(), days: 2 }, line = testLine(trip), total = line.coordinates.length && line.stops.at(-1)!.distance;
        const [from, to] = stopWindow(trip, line, 1)!;
        expect([from / total, to / total]).toEqual([expect.closeTo(.25, 6), expect.closeTo(.75, 6)]);
        const [before, after] = daysAround(trip, line, 1, total * .4);
        expect([before.distance / total, after.distance / total]).toEqual([expect.closeTo(.4, 6), expect.closeTo(.6, 6)]);
        const stops = groupStops([place('Early', 'hotel', 'A', total * .3), place('Middle', 'campsite', 'B', total * .52)], []);
        expect(evenStop(stops, trip, line, 1)?.name).toBe('B');
    });

    it('proposes each open night at the nearest stop with a kind of place of that night, evening out the days left after it', () => {
        const trip = testTrip(), line = testLine(trip);
        const stops = groupStops([
            place('Krone', 'hotel', 'Early', 30), place('Hütte', 'hut', 'Middle', 34, .8), place('Linde', 'hotel', 'Middle', 35),
            place('Adler', 'hostel', 'Middle', 35.5), place('Alp', 'hut', 'Middle', 36.5), place('Seecamping', 'campsite', 'Middle', 37),
            place('Waldcamping', 'campsite', 'Late', 55), place('Post', 'hotel', 'Later', 59),
        ], []);
        const nights = (stay?: Stay[], of: Trip = trip, offered = stops) => [...proposeNights({ ...of, stay }, line, () => offered)]
            .map(([night, p]) => [night, proposalTitle(p), p.km]);
        expect(nights(['camp'])).toEqual([[1, 'Seecamping', 37], [2, 'Waldcamping', 55]]);
        // A hut night ends at the hut nearest the even day end, not at the one nearest the route.
        expect(nights(['hut'])).toEqual([[1, 'Hütte', 34]]);
        expect(nights(['hotel'])).toEqual([[1, 'Around Early', 30], [2, 'Around Later', 59]]);
        // More than one kind ends the day in the town.
        expect(nights(undefined, trip, stops.slice(1))[0]).toEqual([1, 'Around Middle', 35.5]);
        expect(nights(['hotel'], { ...trip, nightStay: { 2: ['camp'] } })).toEqual([[1, 'Around Early', 30], [2, 'Waldcamping', 55]]);
        // A night the rider set stays theirs.
        expect(nights(['hotel'], { ...trip, splits: { 1: .4 } })).toEqual([[2, 'Around Later', 59]]);
    });
});
