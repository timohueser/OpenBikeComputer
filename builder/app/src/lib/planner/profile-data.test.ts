import { expect, it } from 'vitest';
import { profileAscent, profileDescent } from './profile-data';
import type { RoutingLine } from './routing';

it('counts ascent and descent separately and clips changes to the selected day', () => {
    const line = { coordinates: [[8, 48], [8, 48.01], [8, 48.02], [8, 48.03]], elevation: [100, 200, 120, 160] } as RoutingLine;
    expect(profileAscent(0, 1, line)).toBe(140);
    expect(profileDescent(0, 1, line)).toBe(80);
    expect(profileAscent(.5, 1, line)).toBe(40);
    expect(profileDescent(.5, 1, line)).toBe(40);
    line.elevation[1] = null;
    expect(profileAscent(0, 1, line)).toBe(40);
    expect(profileDescent(0, 1, line)).toBe(0);
    expect(profileDescent()).toBe(0);
});
