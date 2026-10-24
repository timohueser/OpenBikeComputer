import { describe, expect, it } from 'vitest';
import { groundList, SLOTS } from './plants';

describe('the ground plants of a region', () => {
    it('keep the commonest when a pack names more than the list holds, the rarer first', () => {
        const plants = Array.from({ length: SLOTS + 4 }, (_, k) => ({ share: k / (SLOTS + 4) }));
        const list = groundList(plants);
        expect(list).toHaveLength(SLOTS);
        expect(list.map(p => p.share)).toEqual(plants.slice(4).map(p => p.share));
    });
});
