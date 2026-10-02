import { describe, expect, it } from 'vitest';
import { profileHeightAt, sampleIndex } from './profile-data';

describe('profile hover lookup', () => {
    it('finds the height between samples, at both ends and next to unknown terrain', () => {
        const samples = [{ progress: 0, height: 100 }, { progress: .5, height: 200 }, { progress: .75, height: null }, { progress: 1, height: 300 }];
        expect(sampleIndex([], .5)).toBe(0);
        expect(profileHeightAt(.5, [])).toBeNull();
        expect(profileHeightAt(.3, [{ progress: 0, height: 120 }])).toBe(120);
        expect(profileHeightAt(0, samples)).toBe(100);
        expect(profileHeightAt(.25, samples)).toBe(150);
        expect(profileHeightAt(.5, samples)).toBe(200);
        expect(profileHeightAt(.6, samples)).toBeNull();
        expect(sampleIndex(samples, 1.2)).toBe(samples.length);
        expect(profileHeightAt(1.2, samples)).toBe(300);
    });
});
