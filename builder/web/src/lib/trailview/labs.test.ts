import { describe, expect, it } from 'vitest';
import { labsOf } from './labs';

describe('the lab registry', () => {
    it('selects the default variant, or the one in the address', () => {
        expect(labsOf('')).toEqual({ terrain: 'cdlod', relief: 'bicubic', pages: 'r32float' });
        expect(labsOf('?lab=terrain:rings,pages:r16&lab=relief:erosion')).toEqual({ terrain: 'rings', relief: 'erosion', pages: 'r16' });
    });

    it('refuses an unknown lab or variant and names the labs', () => {
        expect(() => labsOf('?lab=terrain:naive')).toThrow('The lab "terrain:naive" does not exist. The labs are terrain:cdlod, terrain:rings, relief:bicubic, relief:erosion, pages:r32float, pages:r16.');
        expect(() => labsOf('?lab=water')).toThrow(/does not exist/);
    });
});
