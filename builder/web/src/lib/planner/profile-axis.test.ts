import { describe, expect, it } from 'vitest';
import { axisLeft, axisPixels, axisProgress, axisWidth } from './profile-axis';

describe('profile axis', () => {
    it('places progress on the shown stretch and holds a pointer inside it', () => {
        const axis = { from: .25, to: .75 };
        expect([axisLeft(axis, .5), axisWidth(axis, .25, .5), axisPixels(axis, .5, 800)]).toEqual(['50%', '50%', 400]);
        expect([axisProgress(axis, 400, 800), axisProgress(axis, -10, 800), axisProgress(axis, 900, 800)]).toEqual([.5, .25, .75]);
    });
});
