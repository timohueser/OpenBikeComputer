import { describe, expect, it } from 'vitest';
import { addRestDay, pinNight, planView } from './editor';
import { newVersion, versionSummary } from './versions';
import { testLine, testTrip } from '../../../test-support/planner/trip';

describe('saved versions', () => {
    it('captures the editable plan and a trimmed checkpoint name', () => {
        const trip = testTrip();
        const version = newVersion(trip, undefined, '  Before the pass  ');
        expect(version.trip).toEqual(trip);
        expect(version.name).toBe('Before the pass');
        expect(newVersion(trip, undefined, '  ').name).toBeUndefined();
    });

    it('summarises calendar days, pinned nights and distance', () => {
        const trip = addRestDay(pinNight(testTrip(), undefined, 1, [7.6, 47.24], 'Camp'), 1), line = testLine(trip);
        const distance = planView(trip, line).total.toFixed(1);
        expect(versionSummary(trip, line)).toBe(`4 days · 1 night pinned · ${distance} km`);
        expect(versionSummary({ ...trip, mode: 'route' }, line)).toBe(`Single route · ${distance} km`);
        expect(versionSummary({ ...trip, mode: 'route' }, undefined)).toBe('Single route · Distance pending');
    });
});
