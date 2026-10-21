import { describe, expect, it } from 'vitest';
import { labsOf } from './labs';

describe('the lab registry', () => {
    it('refuses an unknown lab or variant', () => {
        expect(() => labsOf('?lab=terrain:naive')).toThrow('The lab "terrain:naive" does not exist.');
        expect(() => labsOf('?lab=water')).toThrow(/^The lab "water" does not exist\. (The labs are \S.*|There are no labs)\.$/);
    });
});
