import { describe, expect, it } from 'vitest';
import { tree } from './library';
import { fits, tone } from './tone';

describe('the colour matrix of a season', () => {
    it('turns a crown whose season changes only its ramp into that season', () => {
        for (const [model, season] of [['european-beech', 'autumn'], ['sycamore-maple', 'spring'], ['norway-spruce', 'winter']] as const) {
            const summer = tree(model, 'summer', 0), other = tree(model, season, 0), matrix = tone(summer, other)!;
            expect(fits(matrix, summer, other, 8), `${model} ${season}`).toBe(true);
        }
    });

    it('turns no bare crown, and no blossom that the season moves', () => {
        expect(tone(tree('european-beech', 'summer', 0), tree('european-beech', 'winter', 0))).toBeUndefined();
        const summer = tree('hawthorn', 'summer', 0), spring = tree('hawthorn', 'spring', 0);
        expect(fits(tone(summer, spring)!, summer, spring, 8)).toBe(false);
    });
});
