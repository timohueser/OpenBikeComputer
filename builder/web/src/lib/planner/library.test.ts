import { describe, expect, it } from 'vitest';
import { pinNight, addRestDay, setDrawnLeg } from './editor';
import { exportPlan, importPlan, newPlan } from './library';
import { newVersion } from './versions';
import { testTrip } from '../../../test-support/planner/trip';

describe('plan file', () => {
    it('round-trips nights, dates, rest days and drawings with its versions under a new identity', () => {
        const trip = { ...addRestDay(pinNight(testTrip(), undefined, 1, [7.2, 47.5], 'Camp'), 1), startDate: '2026-10-04' };
        const drawn = setDrawnLeg(trip, 'finish', [[7, 47.5]]);
        const plan = newPlan(drawn, 'Autumn tour'), versions = [newVersion(trip, undefined, 'Before the detour')];
        const imported = importPlan(exportPlan(plan, versions));
        expect(imported.plan.id).not.toBe(plan.id);
        expect(imported.plan).toMatchObject({ revision: 0, name: plan.name, trip: JSON.parse(JSON.stringify(drawn)) });
        expect(imported.versions).toMatchObject(JSON.parse(JSON.stringify(versions)));
    });

    it('writes the keys the companion app needs into each plan object, and keeps an open version that the file has', () => {
        const trip = { ...testTrip(), days: 4 }, versions = [newVersion(trip, undefined), newVersion(testTrip(), undefined)];
        const file = JSON.parse(exportPlan({ ...newPlan(trip, 'Tour'), open: versions[0].id }, versions));
        for (const plan of [file.trip, ...file.versions.map((v: { trip: unknown }) => v.trip)]) expect(plan).toMatchObject({ budget: 'days', limit: 0 });
        expect([file.trip.target, ...file.versions.map((v: { trip: { target: number } }) => v.trip.target)]).toEqual([4, 4, 3]);
        expect(importPlan(JSON.stringify(file)).plan.open).toBe(versions[0].id);
        expect(importPlan(JSON.stringify({ ...file, open: 'deleted' })).plan).not.toHaveProperty('open');
        expect(JSON.parse(exportPlan({ ...newPlan(trip, 'Tour'), open: 'deleted' }, versions))).not.toHaveProperty('open');
    });

    it('rejects malformed plans and versions without accepting part of a file', () => {
        const file = JSON.parse(exportPlan(newPlan(testTrip(), 'Valid'), [newVersion(testTrip(), undefined)]));
        for (const invalid of [null, {}, { ...file, version: 99 }, { ...file, trip: {} },
            { ...file, trip: { ...file.trip, startDate: '2026-02-30' } },
            { ...file, trip: { ...file.trip, points: [{ ...file.trip.points[0], anchor: [999, 0] }, file.trip.points[1]] } },
            { ...file, versions: [null] }, { ...file, versions: [file.versions[0], file.versions[0]] },
            { ...file, versions: [{ ...file.versions[0], trip: {} }] }]) {
            expect(() => importPlan(JSON.stringify(invalid))).toThrow();
        }
        expect(() => importPlan('{')).toThrow();
    });
});
