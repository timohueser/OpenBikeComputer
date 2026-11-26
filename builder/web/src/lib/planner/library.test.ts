import { describe, expect, it } from 'vitest';
import { pinNight, setDrawnLeg } from './editor';
import { exportPlan, importPlan, newPlan, planSummary, savedWhen } from './library';
import { testLine, testTrip } from '../../../test-support/planner/trip';

describe('plan file', () => {
    it('round-trips nights, dates, the pace and drawings under a new identity', () => {
        const trip = { ...pinNight(testTrip(), undefined, 1, [7.2, 47.5], 'Camp'), startDate: '2026-10-04', pace: 'easy' as const };
        const plan = newPlan(setDrawnLeg(trip, 'finish', [[7, 47.5]]), 'Autumn tour');
        const imported = importPlan(exportPlan(plan));
        expect(imported.id).not.toBe(plan.id);
        expect(imported).toMatchObject({ name: plan.name, trip: JSON.parse(JSON.stringify(plan.trip)) });
        expect(imported.trip.pace).toBe('easy');
    });

    it('opens an older file with saved versions as its plan, and ignores the versions', () => {
        const plan = newPlan(testTrip(), 'Tour'), file = JSON.parse(exportPlan(plan));
        expect(file).not.toHaveProperty('versions');
        const older = { ...file, open: 'v1', versions: [{ id: 'v1', at: '2026-01-01T09:00:00.000Z', summary: '', trip: file.trip }] };
        const imported = importPlan(JSON.stringify(older));
        expect(imported).toMatchObject({ name: 'Tour', trip: JSON.parse(JSON.stringify(plan.trip)) });
        expect(imported).not.toHaveProperty('open');
    });

    it('writes the riding time at the pace of the plan into its summary', () => {
        const trip = testTrip(), line = testLine(trip);
        expect(planSummary(trip, line).seconds).toBe(Math.round(line.seconds));
        expect(planSummary({ ...trip, pace: 'fast' }, line).seconds).toBe(Math.round(line.seconds * 19 / 27));
    });

    it('writes the keys the companion app needs into the plan object', () => {
        const file = JSON.parse(exportPlan(newPlan({ ...testTrip(), days: 4 }, 'Tour')));
        expect(file.trip).toMatchObject({ budget: 'days', target: 4, limit: 0 });
    });

    it('rejects malformed plans', () => {
        const file = JSON.parse(exportPlan(newPlan(testTrip(), 'Valid')));
        for (const invalid of [null, {}, { ...file, version: 99 }, { ...file, trip: {} },
            { ...file, trip: { ...file.trip, startDate: '2026-02-30' } },
            { ...file, trip: { ...file.trip, points: [{ ...file.trip.points[0], anchor: [999, 0] }, file.trip.points[1]] } }]) {
            expect(() => importPlan(JSON.stringify(invalid))).toThrow();
        }
        expect(() => importPlan('{')).toThrow();
    });
});

describe('saved when', () => {
    it('names today, yesterday and an older day', () => {
        const now = new Date(2026, 9, 14, 20, 0).getTime(), time = (at: Date) => at.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
        const today = new Date(2026, 9, 14, 14, 5), yesterday = new Date(2026, 9, 13, 9, 30), older = new Date(2026, 9, 12, 18, 0);
        expect(savedWhen(today.getTime(), now)).toBe(`Today ${time(today)}`);
        expect(savedWhen(yesterday.getTime(), now)).toBe(`Yesterday ${time(yesterday)}`);
        expect(savedWhen(older.getTime(), now)).toBe(`${older.toLocaleDateString(undefined, { day: 'numeric', month: 'short' })} ${time(older)}`);
    });
});
