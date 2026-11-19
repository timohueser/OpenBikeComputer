import { describe, expect, it } from 'vitest';
import type { CatalogRecord } from '../../lib/planner/signed-routes';
import { shownDescription, signedHref } from './signed-page';

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
