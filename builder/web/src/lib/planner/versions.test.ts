import { describe, expect, it, vi } from 'vitest';
import { addRestDay, pinNight, planView } from './editor';
import { newVersion, versionSummary, writeBack } from './versions';
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

    it('writes a trip back into its version, which keeps its id and name and becomes the newest', () => {
        const trip = testTrip(), edited = addRestDay(trip, 1);
        const older = { ...newVersion(trip, undefined, 'Coast'), at: '2026-01-01T09:00:00.000Z' };
        const newer = { ...newVersion(trip, undefined), at: '2026-01-02T09:00:00.000Z' };
        vi.useFakeTimers({ now: Date.parse('2026-01-03T09:00:00.000Z') });
        const versions = writeBack([newer, older], older.id, edited, undefined);
        vi.useRealTimers();
        expect(versions).toEqual([{ ...older, at: '2026-01-03T09:00:00.000Z', summary: versionSummary(edited, undefined), trip: edited }, newer]);
    });
});
