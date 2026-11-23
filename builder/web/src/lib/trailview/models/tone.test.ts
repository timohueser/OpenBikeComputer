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

    it('does not fit a season whose ramp the matrix turns more than 8 steps off, as the aspen\'s autumn gold', () => {
        const summer = tree('aspen', 'summer', 0), autumn = tree('aspen', 'autumn', 0), matrix = tone(summer, autumn);
        expect(matrix).toBeDefined();
        expect(fits(matrix!, summer, autumn, 8)).toBe(false);
    });

    it('turns no bare crown, and no crown that the season gives flower heads', () => {
        expect(tone(tree('european-beech', 'summer', 0), tree('european-beech', 'winter', 0))).toBeUndefined();
        expect(tone(tree('hawthorn', 'summer', 0), tree('hawthorn', 'spring', 0))).toBeUndefined();
    });
});
