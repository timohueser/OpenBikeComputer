import { describe, expect, it } from 'vitest';
import type { Coordinate } from '../../lib/planner/geo';
import { cumulative } from '../../lib/planner/geo';
import type { CatalogRecord, TrailPart } from '../../lib/planner/signed-routes';
import { borderPoints, stageBorders, stageCells, trailProfileLine, trailRows } from './trail-page';

// A release of one zoom 9 cell, 9-268-180; 9-267-180 is outside it.
const bounds: [number, number, number, number] = [8.4375, 46.8, 9.1, 47.2];
const CELL = '9-268-180';
const ends = { start_udeg: [8_500_000, 46_900_000] as [number, number], end_udeg: [8_600_000, 46_950_000] as [number, number] };
const rowsOf = (parts: TrailPart[], kind: CatalogRecord['kind'] = 'hiking') => trailRows(parts, kind, bounds).map(({ label, numbers, href, gap }) =>
    [label, numbers.join(' · ').replaceAll(' ', ' '), href, gap]);

// The forms of the Swiss catalog. The Via Alpina: stages outside the release before and after the stages in the catalog,
// which add 0 to `end_m`, and stage 69, a gap inside it.
const viaAlpina: TrailPart[] = [
    { id: 1, stage: 1, end_m: 0, missing: true }, { id: 57, stage: 57, end_m: 0, missing: true },
    { id: 58, stage: 58, length_m: 13_300, ascent_m: 898, moving_s: 19_500, hardest: 0, end_m: 13_300, cells: [CELL] },
    { id: 69, stage: 69, length_m: 6_700, end_m: 20_000, missing: true, ...ends },
    { id: 74, stage: 74, length_m: 10_000, ascent_m: 500, end_m: 30_000, cells: ['9-267-180', CELL] },
    { id: 75, stage: 75, end_m: 30_000, missing: true },
];

describe('trailRows', () => {
    it('gives a row for each stage in the catalog and for each gap, and none for a stage outside the release', () => {
        expect(rowsOf(viaAlpina)).toEqual([
            ['Stage 58', '13.3 km · 898 m up · 5h 25m · T1', `#/signed/${CELL}/58`, false],
            ['Stage 69', '6.7 km · Not in the map data', undefined, true],
            ['Stage 74', '10.0 km · 500 m up', `#/signed/${CELL}/74`, false],
        ]);
    });

    it('names the sections of a cut stage, and a section of a trail with no stage numbers', () => {
        // Freiämterweg: stage 3 is cut into two sections.
        const freiamt: TrailPart[] = [
            { id: 1, stage: 1, length_m: 9_000, ascent_m: 100, end_m: 9_000, cells: [CELL] },
            { id: -228255701, stage: 3, length_m: 5_000, ascent_m: 50, end_m: 14_000, cells: [CELL] },
            { id: -228255702, stage: 3, length_m: 6_000, ascent_m: 60, end_m: 20_000, cells: [CELL] },
        ];
        expect(rowsOf(freiamt).map(row => row[0])).toEqual(['Stage 1', 'Stage 3 · Section 1', 'Stage 3 · Section 2']);
        // Grenzpfad Napfbergland: no stage numbers, and its second section is a gap.
        const napf: TrailPart[] = [
            { id: -162503901, length_m: 4_000, ascent_m: 300, end_m: 4_000, cells: [CELL] },
            { id: -162503902, length_m: 3_000, end_m: 7_000, missing: true, ...ends },
        ];
        expect(rowsOf(napf).map(row => [row[0], row[3]])).toEqual([['Section 1', false], ['Section 2', true]]);
    });

    it('gives no grade on a cycling trail', () => {
        const parts: TrailPart[] = [{ id: 5, stage: 1, length_m: 40_000, ascent_m: 400, hardest: 2, end_m: 40_000, cells: [CELL] }];
        expect(rowsOf(parts, 'bicycle')[0][1]).toBe('40.0 km · 400 m up');
    });
});

describe('the profile', () => {
    it('spans the last end_m, and has no border at its ends', () => {
        const line = trailProfileLine({ id: 9, kind: 'hiking', rank: 4, loop: false, length_m: 23_300, ascent_m: 1398, descent_m: 0, cells: [],
            parts: viaAlpina, trail_profile_m: Array.from({ length: 201 }, (_, k) => k === 100 ? null : 400 + k) });
        expect(line.coordinates).toHaveLength(201);
        expect(cumulative(line.coordinates).at(-1)).toBeCloseTo(30, 9);
        expect(line.elevation[100]).toBeNull();
        expect(stageBorders(viaAlpina)).toEqual([13_300 / 30_000, 20_000 / 30_000]);
    });
});

describe('the map', () => {
    it('marks the start of each stage but the first, at the end of a line that runs against the trail', () => {
        const lines = new Map<number, Coordinate[]>([[58, [[8.5, 46.9], [8.6, 46.95]]], [74, [[8.8, 47.1], [8.7, 47.0]]]]);
        const parts = viaAlpina.map(part => part.id === 74 ? { ...part, backwards: true as const } : part);
        expect(borderPoints(parts, id => lines.get(id))).toEqual([[8.7, 47.0]]);
    });

    it('asks for a cell that an earlier stage takes, and for no cell of a stage that the catalog holds', () => {
        // The release also covers 9-268-179, north of the cell; 9-267-180 is outside it.
        const parts: TrailPart[] = [
            { id: 1, end_m: 1, cells: ['9-267-180', CELL] }, { id: 2, end_m: 2, cells: [CELL] }, { id: 3, end_m: 3, cells: ['9-268-179', CELL] },
        ];
        expect(stageCells(parts, () => undefined, bounds)).toEqual([CELL]);
        expect(stageCells(parts, id => id < 3 ? {} as CatalogRecord : undefined, bounds)).toEqual(['9-268-179']);
    });
});
