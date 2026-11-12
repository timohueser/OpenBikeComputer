import { describe, expect, it } from 'vitest';
import { formatGrade, gradeBand, profileGrades } from './grade-data';

describe('terrain grade display', () => {
    it('uses distance, preserves direction and handles uneven sample spacing', () => {
        const samples = [0, .01, .025, .1, .4, .9, 1].map(progress => ({ progress, height: progress * 100 }));
        for (const grade of profileGrades(samples, 1)) expect(grade).toBeCloseTo(10);
        for (const grade of profileGrades(samples.map(sample => ({ ...sample, height: 100 - sample.height })), 1)) expect(grade).toBeCloseTo(-10);
    });
    it('smooths short sample changes without bridging missing terrain', () => {
        const ripple = Array.from({ length: 21 }, (_, i) => ({ progress: i / 20, height: i === 10 ? 1 : 0 }));
        expect(Math.max(...profileGrades(ripple, .2).map(grade => Math.abs(grade!)))).toBeLessThanOrEqual(1);
        const gaps = [{ progress: 0, height: 0 }, { progress: .1, height: 10 }, { progress: .2, height: null }, { progress: .3, height: 400 }, { progress: .5, height: 420 }];
        const grades = profileGrades(gaps, 1);
        expect(grades[0]).toBeCloseTo(10);
        expect(grades.slice(1, 3)).toEqual([null, null]);
        expect(grades[3]).toBeCloseTo(10);
    });
    it('keeps short fragments and zero-length edges unknown, with explicit grade labels', () => {
        expect(profileGrades([{ progress: 0, height: 0 }, { progress: 1, height: 5 }], .01)).toEqual([null]);
        expect(profileGrades([{ progress: 0, height: 0 }, { progress: 0, height: 0 }, { progress: 1, height: 10 }], .1)).toEqual([null, 10]);
        expect(profileGrades([], 0)).toEqual([]);
        expect([-5, 0, 2, 3, 5, 6, 8, 9, 11, 12, 25, -30, null].map(gradeBand)).toEqual([0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 0, 5]);
        expect([2.94, 2.96, 8.94, 8.96].map(gradeBand)).toEqual([0, 1, 2, 3]);
        expect(gradeBand(9.999999)).toBe(gradeBand(10));
        expect(formatGrade(null)).toBe('Grade unknown');
        expect(formatGrade(-.01)).toBe('0.0% grade');
        expect(formatGrade(-10.25)).toBe('−10.2% grade');
        expect(formatGrade(6.1)).toBe('+6.1% grade');
    });
});
