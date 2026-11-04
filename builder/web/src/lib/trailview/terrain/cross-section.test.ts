import { describe, expect, it } from 'vitest';
import ROWS from '../../../../../../host/obc-ground-plan/tests/cross-section.json' with { type: 'json' };
import { crossSection, type Section } from './cross-section';

const VECTOR = ROWS as unknown as ({ kind: string; tags: Record<string, string> } & Section)[];

describe('the cross-section of a way', () => {
    it('gives every row of the vector that the crate asserts too', () => {
        for (const { kind, tags, ...section } of VECTOR) expect(crossSection(kind, tags), `${kind} ${JSON.stringify(tags)}`).toEqual(section);
    });
});
