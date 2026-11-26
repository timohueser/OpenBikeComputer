import { describe, expect, it } from 'vitest';
import { contract, stayKind } from './presentation';

describe('kinds of places to sleep', () => {
    it('gives each place kind that search finds to sleep in one of the five kinds', () => {
        const sleepKinds = Object.values(contract.kinds).filter(kind => kind.parent === 'sleep').flatMap(kind => kind.data);
        expect(sleepKinds.length).toBeGreaterThan(0);
        for (const kind of sleepKinds) expect(stayKind(kind), kind).toBeDefined();
    });
});
