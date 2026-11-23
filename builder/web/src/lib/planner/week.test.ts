import { describe, expect, it } from 'vitest';
import { centredOn, containing, dayLook, end, pinLook, step, weekOf } from './week';
import type { MapPoint } from './map-types';

const range = (from: number, to: number) => Array.from({ length: to - from + 1 }, (_, i) => from + i);

describe('week in focus', () => {
    it('comes with 15 days and searches the night before it', () => {
        expect(weekOf(14, 1)).toBeNull();
        expect(weekOf(15, 1)).toEqual({ start: 1, end: 7, nights: range(1, 7) });
        expect(weekOf(15, 4)).toEqual({ start: 4, end: 10, nights: range(3, 10) });
    });

    it('moves inside the trip', () => {
        expect([centredOn(32, 2), centredOn(32, 20), centredOn(32, 31)]).toEqual([1, 17, 26]);
        expect([step(32, 24, 7), step(32, 3, -7), step(32, 10, 1)]).toEqual([26, 1, 11]);
        // The least move: a day before the week starts it, a day after it ends it, a day inside moves nothing.
        expect([containing(32, 8, 3), containing(32, 8, 20), containing(32, 8, 14)]).toEqual([3, 14, 8]);
        expect(end(32)).toBe(26);
    });

    it('holds a start past the end inside a trip that has fewer days now', () => {
        expect(weekOf(20, 26)).toMatchObject({ start: 14, end: 20 });
    });

    it('draws the days and pins of the week on the map, and the rest quiet', () => {
        const week = weekOf(20, 8)!;
        expect([8, 14, 7, 15].map(day => dayLook(week, null, day))).toEqual(['colour', 'colour', 'quiet', 'quiet']);
        expect(dayLook(week, 3, 3)).toBe('lit');
        expect(range(1, 20).every(day => dayLook(null, null, day) === 'colour')).toBe(true);
        const pin = (kind: MapPoint['kind'], extra: Partial<MapPoint> = {}): MapPoint => ({ id: kind, coordinate: [7, 47], label: kind, kind, ...extra });
        // The night before the week and its last night stay marks; the nights around them become dots.
        expect([7, 14, 6, 15].map(night => pinLook(week, pin('dayend', { night })))).toEqual(['shown', 'shown', 'dot', 'dot']);
        expect([9, 3].map(day => pinLook(week, pin('via', { day })))).toEqual(['shown', 'hidden']);
        const others = [pin('start', { day: 1 }), pin('finish', { day: 20 }), pin('place')];
        expect(others.map(point => pinLook(week, point))).toEqual(['shown', 'shown', 'shown']);
        expect([...others, pin('dayend', { night: 2 }), pin('via', { day: 3 })].every(point => pinLook(null, point) === 'shown')).toBe(true);
    });
});
