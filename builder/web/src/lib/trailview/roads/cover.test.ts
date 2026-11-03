import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { COVER_MARGIN, COVER_ROW, COVER_TILE } from './cover';
import { FINE_SHIFT, lookWord } from './piece';
import { CF, LOOK, MAT } from './section';

const read = (path: string) => readFileSync(new URL(path, import.meta.url), 'utf8');
const number = (source: string, name: string) => Number(new RegExp(`const ${name}: [fiu]32 = (-?[\\d.]+)u?;`).exec(source)![1]);

describe('the cover pass', () => {
    it('reads the vertex as piece.ts packs it, and draws the tiles that the snap pass reads', () => {
        const cover = read('./cover.wgsl'), stamp = read('../terrain/stamp.wgsl');
        expect(number(cover, 'CORRIDOR_STEP')).toBeCloseTo(1 / 12, 6);
        expect(number(cover, 'FINE_SHIFT')).toBe(FINE_SHIFT);
        expect((lookWord(0, 0, CF.DECK | CF.FILL, 0) >>> number(cover, 'FLAGS_SHIFT')) & number(cover, 'FLAGS_BITS')).toBe(CF.DECK | CF.FILL);
        expect(['GROUND', 'RAIL'].map(name => number(cover, `MAT_${name}`))).toEqual([MAT.GROUND, MAT.RAIL]);
        expect(['GRAVEL', 'TRAIL'].map(name => number(cover, `LOOK_${name}`))).toEqual([LOOK.GRAVEL, LOOK.TRAIL]);
        expect(['FILL', 'DECK'].map(name => number(cover, `CF_${name}`))).toEqual([CF.FILL, CF.DECK]);
        expect([number(cover, 'TILE'), number(cover, 'MARGIN')]).toEqual([COVER_TILE, COVER_MARGIN]);
        expect(['COVER_TILE', 'COVER_MARGIN', 'COVER_ROW'].map(name => number(stamp, name))).toEqual([COVER_TILE, COVER_MARGIN, COVER_ROW]);
        expect(['COVER_LOWEST_M', 'COVER_SPAN_M'].map(name => number(stamp, name))).toEqual(['LOWEST_M', 'SPAN_M'].map(name => number(cover, name)));
    });
});
