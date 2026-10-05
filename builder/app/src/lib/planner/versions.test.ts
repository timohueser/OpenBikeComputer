import { describe, expect, it } from 'vitest';
import { addRestDay, pinNight, planView } from './editor';
import { newVersion, versionSummary } from './versions';
import { routed, testTrip } from '../../../test-support/planner/trip';

describe('saved versions', () => {
    it('captures the editable plan and a trimmed checkpoint name', () => {
        const trip = testTrip();
        const version = newVersion(trip, '  Before the pass  ');
        expect(version.trip).toEqual(trip);
        expect(version.name).toBe('Before the pass');
        expect(newVersion(trip, '  ').name).toBeUndefined();
    });

    it('summarises calendar days, pinned nights and distance', () => {
        const trip = routed(addRestDay(pinNight(testTrip(), 1, [7.6, 47.24], 'Camp'), 1));
        const distance = planView(trip).total.toFixed(1);
        expect(versionSummary(trip)).toBe(`4 days · 1 night pinned · ${distance} km`);
        expect(versionSummary({ ...trip, mode: 'route' })).toBe(`Single route · ${distance} km`);
        expect(versionSummary({ ...testTrip(), mode: 'route' })).toBe('Single route · Distance pending');
    });
});
