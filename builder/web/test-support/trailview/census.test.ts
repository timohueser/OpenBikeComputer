import { describe, expect, it } from 'vitest';
import config from '../../tests/browser/trailview/fixture/config.json' with { type: 'json' };
import { worldPixel } from '../../src/lib/planner/layers/mercator';
import { UNITS } from '../../src/lib/trailview/frame/space';
import { lookWord, PieceBuilder } from '../../src/lib/trailview/roads/piece';
import { LOOK } from '../../src/lib/trailview/roads/section';
import { quad } from '../../src/lib/trailview/roads/sweep';
import { metresPerUnit, PROFILES, type Line } from '../../src/lib/trailview/terrain/bins';
import { DEEPEST } from '../../src/lib/trailview/stream/features';
import { CHILD, size } from '../../src/lib/trailview/terrain/lod';
import { corners, coveredBy, splitsCorners, tornFaces, type WayOut } from './census';

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

    it('leave out a footway: one between two sidewalks does not split their corner', () => {
        const line = (name: string) => ({ profile: Object.keys(PROFILES).indexOf(name) }) as Line;
        expect([splitsCorners(line('footway')), splitsCorners(line('residential'))]).toEqual([false, true]);
    });

    it('split a corner at a way out between them that can have a sidewalk, also with none', () => {
        const between: WayOut = { angle: (3 * Math.PI) / 4, left: -1, right: -1 };
        expect(joins([west(-1, 1), between, north(2, -1)], 1, 2)).toBe(false);
    });
});

// A piece in the middle of an L16 square at the middle of the fixture.
const [cx, cy] = worldPixel([(config.bounds[0] + config.bounds[2]) / 2, (config.bounds[1] + config.bounds[3]) / 2], UNITS);
const side = size(CHILD), x0 = (Math.floor(cx / side) + 0.5) * side, y0 = (Math.floor(cy / side) + 0.5) * side, unit = metresPerUnit(y0);
const word = lookWord(LOOK.ASPHALT, 0, 0, 0, 3);

describe('the gaps', () => {
    it('take no cover from a face whose three vertices are one point, also where it comes before a real face', () => {
        // Points east and south of the square's middle, in one cell of the index of faces.
        const b = new PieceBuilder(x0, y0), at = (east: number, south: number) => b.vertex(x0 + east / unit, y0 + south / unit, 0, word, 0, 0);
        b.near.push(at(1, 1), at(1, 1), at(1, 1), at(2, 2), at(4, 2), at(2, 4));
        const tile = { level: DEEPEST, x: Math.floor(x0 / size(DEEPEST)), y: Math.floor(y0 / size(DEEPEST)) };
        const covered = coveredBy([{ tile, list: [], meshes: [{ pieces: [b.piece(0)] }] }]), sample = (east: number, south: number) => covered(x0 + east / unit, y0 + south / unit);
        expect([sample(1, 3), sample(2.5, 2.5), sample(1, 3), sample(3.5, 3.5)]).toEqual([false, true, false, false]);
    });
});

describe('the torn faces', () => {
    /** The kinds' counts of the quads of a carriageway, each a list of rows from left to right of points east, north and up in metres. */
    const counts = (stations: number[][][]) => {
        const b = new PieceBuilder(x0, y0);
        const ids = stations.map(rows => rows.map(([east, north, up]) => b.vertex(x0 + east / unit, y0 - north / unit, up, word, 0, 0)));
        for (let j = 0; j + 1 < ids.length; j++) for (let c = 0; c + 1 < ids[j].length; c++) quad(b.near, ids[j][c], ids[j][c + 1], ids[j + 1][c + 1], ids[j + 1][c], false);
        return Object.fromEntries(Object.entries(tornFaces([b.piece(0)])).map(([kind, points]) => [kind, points.length / 2]));
    };
    const flat = [[[-3, 0, 0], [3, 0, 0]], [[-3, 1, 0], [3, 1, 0]]];

    it('find a fold where the left edge of a quad crosses its right edge, as inside a hairpin', () => {
        expect(counts([flat[0], [[-3, 1, 0], [-4, 1, 0]]]).folds).toBe(2);
    });

    it('find a sliver where a quad is 1 cm wide in plan and 10 cm high', () => {
        expect(counts([[[0, 0, 0], [0.01, 0, 0.1]], [[0, 1, 0], [0.01, 1, 0.1]]]).slivers).toBe(2);
    });

    it('find a twist where one corner of a quad lies 1.5 m over the other three', () => {
        expect(counts([flat[0], [[-3, 1, 0], [3, 1, 1.5]]])).toEqual({ folds: 0, slivers: 0, twists: 1 });
    });

    it('find none in a banked, crowned bend of 8 m radius', () => {
        const bend = Array.from({ length: 13 }, (_, j) => {
            const turn = (j / 12) * (Math.PI / 2);
            return [[-3, 0.1], [0, 0.18], [3, 0.34]].map(([a, up]) => [(8 + a) * Math.cos(turn), (8 + a) * Math.sin(turn), up + 0.2 * j]);
        });
        expect(counts(bend)).toEqual({ folds: 0, slivers: 0, twists: 0 });
    });
});
