import { describe, expect, it } from 'vitest';
import { labsOf } from './labs';

describe('the lab registry', () => {
    it('selects the default variant, or the one in the address', () => {
        expect(labsOf('')).toEqual({ terrain: 'naive' });
        expect(labsOf('?lab=terrain:naive')).toEqual({ terrain: 'naive' });
    });

    it('refuses an unknown lab or variant and names the labs', () => {
        expect(() => labsOf('?lab=terrain:cdlod')).toThrow('The lab "terrain:cdlod" does not exist. The labs are terrain:naive.');
        expect(() => labsOf('?lab=water')).toThrow(/does not exist/);
    });
});
