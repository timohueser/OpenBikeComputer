import { describe, expect, it } from 'vitest';
import TABLE from '../../../../../../data/trailview/kinds.json' with { type: 'json' };
import type { Feature } from '../stream/features';
import { ANCHOR_WORDS, POINT } from '../structures/anchored';
import { OWNER, structureMeshes } from '../structures/node';
import { KINDS } from '.';

describe('kinds', () => {
    it('draws only kinds of the table', () => {
        const names = new Set(TABLE.map(entry => entry.kind));
        expect([...KINDS.keys()].filter(name => !names.has(name))).toEqual([]);
    });

    it('stands a station at the end of a lift, not where its tile cuts it', () => {
        // A chair lift from inside its tile past the east edge, with a pylon between; the node holds both.
        const tile = { level: 14, x: 8600, y: 5800 }, node = { level: OWNER, x: 2 * tile.x, y: 2 * tile.y };
        const lift: Feature = { kind: 'aerialway', tags: { aerialway: 'chair_lift' }, geometry: 'line', parts: [{ outer: false, xy: new Int32Array([100, 100, 4000, 100, 9000, 100]), z: null }] };
        const { anchors, far } = structureMeshes([lift], tile, node);
        const words = new Uint32Array(anchors!), flags = Array.from({ length: words.length / ANCHOR_WORDS }, (_, i) => words[i * ANCHOR_WORDS + 7]);
        // The station and the pylon are points; the two spans hang the cables.
        expect(flags.filter(f => f & POINT).length).toBe(2);
        expect(flags.filter(f => !(f & POINT)).length).toBe(2);
        expect(far.anchored).not.toBeNull();
    });
});
