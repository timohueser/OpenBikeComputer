import { describe, expect, it } from 'vitest';
import { corners, type WayOut } from './census';

/** Whether the corners about a vertex join sidewalks a and b. */
const joins = (around: WayOut[], a: number, b: number) => corners(around).some(([p, q]) => (p === a && q === b) || (p === b && q === a));

describe('the corners of the short-sidewalk count', () => {
    // The cases of `a_piece_sums_its_length_around_a_corner_and_not_across_the_road` of the bake's
    // host/obc-trailview-features/src/roads/sidewalks.rs. A way runs east into a vertex and one runs north out
    // of it: away from the vertex, the first points west and its own left, the north, is its away-right.
    const west = (left: number, right: number): WayOut => ({ angle: Math.PI, left, right });
    const north = (left: number, right: number): WayOut => ({ angle: Math.PI / 2, left, right });

    it('join the left of a way east and the left of a way north, which face one corner', () => {
        expect(joins([west(-1, 1), north(2, -1)], 1, 2)).toBe(true);
    });

    it('join no sidewalks across the road', () => {
        expect(corners([west(-1, 1), north(-1, 3)]).filter(([a, b]) => a >= 0 && b >= 0)).toEqual([]);
    });

    it('split a corner at a way out between them that can have a sidewalk, also with none', () => {
        const between: WayOut = { angle: (3 * Math.PI) / 4, left: -1, right: -1 };
        expect(joins([west(-1, 1), between, north(2, -1)], 1, 2)).toBe(false);
    });
});
