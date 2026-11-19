import { describe, expect, it } from 'vitest';
import type { CatalogRecord } from '../../lib/planner/signed-routes';
import { partLabel, shownDescription, signedHref, stagePart } from './signed-page';

const record = (fields: Partial<CatalogRecord>): CatalogRecord => ({
    id: 1, kind: 'hiking', rank: 1, loop: false, length_m: 1000, ascent_m: 0, descent_m: 0, cells: ['9-268-180'], ...fields,
});

describe('shownDescription', () => {
    const long = record({ id: 50, name: 'Waldstätterweg' });
    const recordOf = (id: number) => id === long.id ? long : undefined;
    const cases: [description: string, fields: Partial<CatalogRecord>, shown: boolean][] = [
        ['Etappe 3', { name: 'Weg der Schweiz' }, false],
        ['3. Etappe des Waldstätterwegs', { name: 'Waldstätterweg - Etappe 3', parent: 50 }, false],
        ['markierter Wanderweg', { name: 'La Goule' }, false],
        ['Trockener Steg - Hirli - Schwarzsee', { name: 'Matterhorn glacier trail' }, false],
        ['WHR_main Stage 9 La Sage - Lac de Moiry', { name: 'Walker’s Haute Route Stage 9' }, false],
        ['Wanderweg von Isleten über Gitschenen & Hinter Jochli zur Klewenalp.', { name: 'ViaUrschweiz' }, true],
        ['Nur bei absoluter Tritt- und Schwindelfreiheit zu begehen!', { name: 'Fründenhütte' }, true],
        ['Asphalt: 18 km Naturbelag: 6 km davon Singletrail: 0 km', { name: 'Goldi-Route' }, true],
    ];

    it.each(cases)('"%s" shows only when it says something that the record does not', (description, fields, shown) => {
        expect(shownDescription(record({ ...fields, description }), recordOf)).toBe(shown ? description : undefined);
    });
});

describe('signedHref', () => {
    // A release of one zoom 9 cell, 9-268-180.
    const bounds: [number, number, number, number] = [8.4375, 46.8, 9.1, 47.2];

    it('names the first cell of the record that the release covers', () => {
        expect(signedHref(record({ id: 7, cells: ['9-267-180', '9-268-180'] }), bounds)).toBe('#/signed/9-268-180/7');
    });

    it('gives no address when the release covers no cell of the record', () => {
        expect(signedHref(record({ cells: ['9-267-180'] }), bounds)).toBeUndefined();
    });
});

describe('stagePart', () => {
    // 100 is a complete trail of two stages; 250 a long route of one stage inside it; 300 a trail of five stages.
    const longs = [record({ id: 100, name: 'Grand', stages: [1, 6] }), record({ id: 250, name: 'Inner', stage_count: 1 }),
        record({ id: 300, name: 'Five', stage_count: 5 })];
    const recordOf = (id: number) => longs.find(long => long.id === id);

    it('names a stage in its trail, and a stage of a long route of one stage in its parent', () => {
        expect(stagePart(record({ trail: 300, trail_stage: 2 }), recordOf)).toMatchObject({ part: 'Stage 2 of 5', trailId: 300 });
        const inner = stagePart(record({ id: 6, trail: 250, trail_stage: 1, parent: 100, stage: 2 }), recordOf);
        expect(inner).toMatchObject({ part: 'Stage 2 of 2', byParent: true, trailId: 100 });
        expect(inner.long?.name).toBe('Grand');
    });
});

describe('partLabel', () => {
    it('names a section of a cut stage, a section, a stage, and else a part by its name', () => {
        expect([partLabel(8, 2), partLabel(undefined, 2), partLabel(8), partLabel(undefined, undefined, 'Etappe 1'), partLabel()])
            .toEqual(['Stage 8 · Section 2', 'Section 2', 'Stage 8', 'Etappe 1', false]);
    });
});
