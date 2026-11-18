import { describe, expect, it } from 'vitest';
import type { TrailPart } from '../../lib/planner/signed-routes';
import { stageCount, stripCells } from './strip';

describe('stripCells', () => {
    it('makes each part as wide as its length, and a part with no length as wide as the mean', () => {
        // Two missing stages with no length at the start, then 10 km and 30 km, and a missing stage of 20 km.
        const parts: TrailPart[] = [
            { id: 1, stage: 1, end_m: 0, missing: true }, { id: 2, stage: 2, end_m: 0, missing: true },
            { id: 3, stage: 3, end_m: 10_000 }, { id: 4, stage: 4, end_m: 40_000 }, { id: 5, stage: 5, end_m: 60_000, missing: true },
        ];
        // The mean of 10, 30 and 20 km is 20 km, so the strip is 100 km wide.
        expect(stripCells(parts, true)).toEqual([
            { id: 1, share: 0.2, number: 1, missing: true, jump: false }, { id: 2, share: 0.2, number: 2, missing: true, jump: false },
            { id: 3, share: 0.1, number: 3, missing: false, jump: false }, { id: 4, share: 0.3, number: 4, missing: false, jump: false },
            { id: 5, share: 0.2, number: 5, missing: true, jump: false },
        ]);
        expect([stageCount(5, parts), stageCount(2, parts.slice(2, 4)), stageCount(5), stageCount(1)]).toEqual(['2 of 5 stages', '2 stages', '5 stages', false]);
    });

    it('numbers no part of a trail with fewer than two stages, and marks a jump', () => {
        const sections: TrailPart[] = [{ id: -101, end_m: 50_000, cut: true }, { id: -102, end_m: 100_000, jump: true }];
        expect(stripCells(sections, false)).toEqual([
            { id: -101, share: 0.5, missing: false, jump: false }, { id: -102, share: 0.5, missing: false, jump: true },
        ]);
    });
});
