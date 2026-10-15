import { describe, expect, it } from 'vitest';
import { groupStops, stopWindow, daysAround, evenStop } from './stops';
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
        expect(stops[0].shop.map(p => p.place.label)).toEqual(['Schmidts Markt']);
        expect(stops[0].food.map(p => p.place.label)).toEqual(['Löwen']);
        expect(stops[2].food.map(p => p.place.label)).toEqual(['Restaurant am Riedsee']);
    });

    it('offers stops half a day either side of the even day end, with both days around each', () => {
        const trip = { ...testTrip(), days: 2, limit: 30 }, line = testLine(trip), total = line.coordinates.length && line.stops.at(-1)!.distance;
        const [from, to] = stopWindow(trip, line, 1)!;
        // The distance target does not narrow the stretch: a longer day is a choice that shows its length.
        expect([from / total, to / total]).toEqual([expect.closeTo(.25, 6), expect.closeTo(.75, 6)]);
        const [before, after] = daysAround(trip, line, 1, total * .4);
        expect([before.distance / total, after.distance / total]).toEqual([expect.closeTo(.4, 6), expect.closeTo(.6, 6)]);
        const stops = groupStops([place('Early', 'hotel', 'A', total * .3), place('Middle', 'campsite', 'B', total * .52)], []);
        expect(evenStop(stops, trip, line, 1)?.name).toBe('B');
    });
});
