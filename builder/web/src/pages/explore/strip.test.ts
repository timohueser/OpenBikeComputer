import { describe, expect, it } from 'vitest';
import type { TrailPart } from '../../lib/planner/signed-routes';
import { heldStages, stageCount, stripCells } from './strip';

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
        expect([stageCount(5, parts), stageCount(2, parts.slice(2, 4)), stageCount(3, parts.slice(0, 2)), stageCount(5), stageCount(1)])
            .toEqual(['2 of 5 stages', '2 stages', '3 stages, none listed', '5 stages', false]);
    });

    it('counts a stage number that no part has as held, and one that only a missing part has as not held', () => {
        // One official route stands for the stages 3 to 5 of a trail of six.
        const parts: TrailPart[] = [{ id: 1, stage: 1, end_m: 1 }, { id: 2, stage: 2, end_m: 2 }, { id: -1000000056100, stage: 3, end_m: 3 },
            { id: 6, stage: 6, end_m: 4 }];
        const missing = parts.map((part): TrailPart => part.id === 6 ? { ...part, missing: true } : part);
        expect([heldStages(parts), stageCount(6, parts), stageCount(6, missing)]).toEqual([6, '6 stages', '5 of 6 stages']);
    });

    it('numbers no part of a trail with fewer than two stages, and marks a jump', () => {
        const sections: TrailPart[] = [{ id: -101, end_m: 50_000, cut: true }, { id: -102, end_m: 100_000, jump: true }];
        expect(stripCells(sections, false)).toEqual([
            { id: -101, share: 0.5, missing: false, jump: false }, { id: -102, share: 0.5, missing: false, jump: true },
        ]);
    });
});
