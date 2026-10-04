import { describe, expect, it } from 'vitest';
import { addRestDay, coordinateAt, cumulative, initialTrip, pinNight, routeCoordinates } from './editor';
import { newVersion, versionSummary } from './versions';

describe('saved versions', () => {
    it('captures the editable plan and a trimmed checkpoint name', () => {
        const trip = initialTrip();
        const version = newVersion(trip, '  Before the pass  ');
        expect(version.trip).toEqual(trip);
        expect(version.name).toBe('Before the pass');
        expect(newVersion(trip, '  ').name).toBeUndefined();
    });

    it('summarises calendar days, pinned nights and distance', () => {
        const initial = initialTrip();
        const trip = addRestDay(pinNight(initial, 1, coordinateAt(routeCoordinates(initial), .4), 'Camp'), 1);
        const distance = cumulative(routeCoordinates(trip)).at(-1)!.toFixed(1);
        expect(versionSummary(trip)).toBe(`4 days · 1 night pinned · ${distance} km`);
        expect(versionSummary({ ...initial, mode: 'route' })).toBe(`Single route · ${cumulative(routeCoordinates(initial)).at(-1)!.toFixed(1)} km`);
    });
});
