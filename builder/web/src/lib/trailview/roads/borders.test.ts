import { describe, expect, it } from 'vitest';
import { borders } from './borders';
import { lookWord } from './piece';
import { CF, LOOK, MAT } from './section';

const GROUND = lookWord(MAT.GROUND, 0, 0, 0);

/** The borders of triangles given as plan points in steps (x, y per corner, three corners per face), each with `word`: as "x,y-x,y", lesser end first. */
function bordersOf(faces: number[][], word = GROUND): string[] {
    const xy = Int32Array.from(faces.flat()), words = new Uint32Array(xy.length / 2).fill(word), indices = Array.from({ length: xy.length / 2 }, (_, i) => i);
    const out = borders(xy, words, indices), lines: string[] = [];
    for (let i = 0; i < out.length; i += 4) {
        const [a, b] = [[out[i], out[i + 1]], [out[i + 2], out[i + 3]]].sort((p, q) => p[0] - q[0] || p[1] - q[1]);
        lines.push(`${a}-${b}`);
    }
    return lines.sort();
}

/** The summed length of borders. */
const length = (lines: string[]) => lines.reduce((sum, line) => {
    const [ax, ay, bx, by] = line.split(/[-,]/).map(Number);
    return sum + Math.hypot(bx - ax, by - ay);
}, 0);

/** A square from (x, y) of side `s` as two faces. */
const square = (x: number, y: number, s: number) => [[x, y, x + s, y, x + s, y + s], [x, y, x + s, y + s, x, y + s]];

/** One face left of x = 10 from y = 0 to 10, and three right of it that meet it at (10 + dx, 4). */
const split = (dx: number) => [[0, 0, 10, 0, 10, 10], [10, 0, 20, 0, 10 + dx, 4], [10 + dx, 4, 20, 0, 20, 10], [10 + dx, 4, 20, 10, 10, 10]];

describe('the borders of a cover', () => {
    it('are the outer edges of faces that share their inner edges', () => {
        expect(bordersOf(square(0, 0, 10))).toEqual(['0,0-0,10', '0,0-10,0', '0,10-10,10', '10,0-10,10']);
        expect(bordersOf([...square(0, 0, 10), ...square(10, 0, 10)])).not.toContain('10,0-10,10');
    });

    it('lie on both sides of a crack of a step', () => {
        const lines = bordersOf([...square(0, 0, 10), ...square(11, 0, 10)]);
        expect(lines).toContain('10,0-10,10');
        expect(lines).toContain('11,0-11,10');
        // A vertex a step off the line leaves a sliver open: its two edges and the line border it.
        expect(bordersOf(split(1))).toEqual(expect.arrayContaining(['10,0-10,10', '10,0-11,4', '10,10-11,4']));
    });

    it('leave out an edge that edges on its line pair up with, and the parts of an edge inside another face', () => {
        expect(bordersOf(split(0))).toEqual(['0,0-10,0', '0,0-10,10', '10,0-20,0', '10,10-20,10', '20,0-20,10']);
        expect(bordersOf([...square(0, 0, 30), ...square(10, 10, 5)])).toEqual(['0,0-0,30', '0,0-30,0', '0,30-30,30', '30,0-30,30']);
        // Two squares that overlap: their outline alone, 50 steps.
        const lines = bordersOf([...square(0, 0, 10), ...square(5, 0, 10)]);
        expect(length(lines)).toBe(50);
        expect(lines.filter(line => line.split(/[-,]/).map(Number).every((c, i) => (i % 2 ? c > 0 && c < 10 : c > 0 && c < 15)))).toEqual([]);
    });

    it('come from no detail and no deck', () => {
        expect(bordersOf(square(0, 0, 10), lookWord(MAT.RAIL, 0, 0, 0))).toEqual([]);
        expect(bordersOf(square(0, 0, 10), lookWord(LOOK.ASPHALT, 0, CF.DECK, 0))).toEqual([]);
    });
});
