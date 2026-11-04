import { describe, expect, it } from 'vitest';
import VECTOR from '../../../../../../host/obc-ground-plan/tests/cross-section.json' with { type: 'json' };
import { crossSection } from './cross-section';

describe('the cross-section of a way', () => {
    it('gives every row of the vector that the crate asserts too', () => {
        for (const { kind, tags, ...section } of VECTOR) expect(crossSection(kind, tags as Record<string, string>), `${kind} ${JSON.stringify(tags)}`).toEqual(section);
    });
});
