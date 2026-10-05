import { describe, expect, it } from 'vitest';
import { profileAscent, profileDescent, profileHeightAt, sampleIndex } from './profile-data';
import type { RoutingLine } from './routing';

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

it('counts ascent and descent separately and clips changes to the selected day', () => {
    const coordinates = [[8, 48], [8, 48.01], [8, 48.02], [8, 48.03]];
    const line = { coordinates, elevation: [100, 200, 120, 160] } as RoutingLine;
    expect(profileAscent(0, 1, line)).toBe(140);
    expect(profileDescent(0, 1, line)).toBe(80);
    expect(profileAscent(.5, 1, line)).toBe(40);
    expect(profileDescent(.5, 1, line)).toBe(40);
    const gap = { coordinates, elevation: [100, null, 120, 160] } as RoutingLine;
    expect(profileAscent(0, 1, gap)).toBe(40);
    expect(profileDescent(0, 1, gap)).toBe(0);
    expect(profileDescent()).toBe(0);
});
