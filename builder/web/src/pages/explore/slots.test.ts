import { describe, expect, it } from 'vitest';
import type { FactsPlace } from '../../lib/planner/signed-routes';
import { defaultQuery, type Query } from './search';
import { effectiveQuery, resetSlot, slotSet, slotWords } from './slots';

const TODAY = '2026-07-01';
const PLACES: Record<string, FactsPlace> = { w2: { kind: 'parking', size: 'large', name: 'Talboden', at_udeg: [0, 0] }, n4: { kind: 'peak', name: 'Pilatus', at_udeg: [0, 0] } };
const place = (id: string) => PLACES[id];
const query = (change: Partial<Query> = {}): Query => ({ ...defaultQuery(TODAY), ...change });
/** The words with plain spaces: the app writes a non-breaking space before a unit. */
const plain = (words: Record<string, string[]>) => Object.fromEntries(Object.entries(words).map(([key, list]) => [key, list.map(word => word.replaceAll('\u00a0', ' '))]));
const full = query({ start: [8, 47], car: true, drive: 30, lift: 'always', shapes: ['loop', 'liftUp'], length: [6000, 16_000], climb: [0, 600],
    grade: [1, 2], date: '2026-12-12', wishes: { peak: 'must', lake: 'nice', snow: 'must', 'via:n4': 'must' } });

describe('the words of the slots', () => {
    it('are empty for the default query, so each slot shows its default', () => {
        expect(slotWords(query(), place, TODAY)).toEqual({ from: [], hike: [], way: [] });
    });

    it('name each value of a slot in a fixed order, a nice wish "if possible", and the day of "Free of snow"', () => {
        expect(plain(slotWords(full, place, TODAY))).toEqual({
            from: ['Start point', 'By car, up to 30 km', 'Always a lift'],
            hike: ['Loop', 'Lift up', '6 km to 16 km', 'Under 600 m climb', 'A T2 part, up to T3'],
            way: ['Summit', 'Lake if possible', 'Free of snow on 12 Dec', 'Passes Pilatus'],
        });
    });

    it('name a pinned start and the walk from it, and lifts with no start point', () => {
        expect(plain(slotWords(query({ pin: 'w2', walk: 500 }), place, TODAY)).from).toEqual(['Talboden', 'Walk up to 500 m']);
        expect(slotWords(query({ lift: 'helps' }), place, TODAY).from).toEqual(['Anywhere on the map', 'Lifts']);
    });
});

describe('a slot', () => {
    it('holds a value that shows no word, such as the car with no start point', () => {
        expect(slotSet(query({ car: true }), 'from', TODAY)).toBe(true);
        expect(slotWords(query({ car: true }), place, TODAY).from).toEqual([]);
        expect(slotSet(query(), 'from', TODAY)).toBe(false);
    });

    it('resets only its own fields', () => {
        const reset = resetSlot(full, 'way', TODAY);
        expect(reset).toEqual({ ...full, wishes: {}, date: TODAY });
        expect(resetSlot(full, 'hike', TODAY).start).toEqual([8, 47]);
    });

    it('that goes off searches with its defaults and shows no words', () => {
        const searched = effectiveQuery(full, { from: true, way: true, trails: false }, TODAY);
        // Lifts go off with "From", so the lift shape no longer counts.
        expect(plain(slotWords(searched, place, TODAY))).toEqual({ from: [], hike: ['Loop', '6 km to 16 km', 'Under 600 m climb', 'A T2 part, up to T3'], way: [] });
        expect(searched.lift).toBeUndefined();
        // The long trails tab keeps only the upper grade of "Hike".
        expect(slotWords(effectiveQuery(full, { from: true, way: true, trails: true }, TODAY), place, TODAY).hike).toEqual(['Up to T3']);
    });
});
