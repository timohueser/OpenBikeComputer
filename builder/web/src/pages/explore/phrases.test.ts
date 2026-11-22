import { describe, expect, it, onTestFinished } from 'vitest';
import type { CatalogRecord, FactsPlace } from '../../lib/planner/signed-routes';
import { settings } from '../../lib/settings/settings.svelte';
import { scan, suggest } from '../../lib/ui/search-bar/read';
import { apply, EXAMPLES, phraseTable } from './phrases';
import { defaultQuery, type Loaded } from './search';
import { slotWords } from './slots';

const TODAY = '2026-07-01';
const PLACES: Record<string, FactsPlace> = { n4: { kind: 'peak', name: 'Pilatus', at_udeg: [0, 0] }, w2: { kind: 'parking', name: 'Talboden', at_udeg: [0, 0] } };
const record: CatalogRecord = { id: 1, kind: 'hiking', rank: 1, loop: true, length_m: 8000, ascent_m: 500, descent_m: 500, cells: [] };
const loaded: Loaded = {
    records: [record], place: id => PLACES[id],
    factsOf: () => ({ places: [['n4', 0]], parking: [['w2', 100]], paved_pct: 0, big_road_pct: 0, water_pct: 0 }),
};
/** The words with plain spaces: the app writes a non-breaking space before a unit. */
const plain = (words: string[]) => words.map(word => word.replaceAll(' ', ' '));

/** The text after Enter: the words of the slots, and the words that stay in the field with their notes. */
function enter(text: string) {
    const parts = scan(text, phraseTable(loaded, {}));
    const words = slotWords(apply(defaultQuery(TODAY), parts.flatMap(part => part.kind === 'value' ? [part.value] : [])), loaded.place, TODAY);
    return {
        from: plain(words.from), hike: plain(words.hike), way: plain(words.way),
        stays: parts.flatMap(part => part.kind === 'unknown' ? [[text.slice(part.from, part.to), part.note]] : []),
    };
}
const labels = (fragment: string) => plain(suggest(fragment, phraseTable(loaded, {})).map(choice => choice.label));

describe('the words of Explore', () => {
    it('send each phrase of the examples to its slot, in any order', () => {
        expect(EXAMPLES.map(enter)).toEqual([
            { from: [], hike: ['Loop', 'Under 15 km'], way: ['Hut with food'], stays: [] },
            { from: [], hike: ['Up to T3'], way: ['Summit', 'Mostly open'], stays: [] },
            { from: [], hike: ['Under 800 m climb'], way: ['Lake', 'Waterfall'], stays: [] },
        ]);
        expect(enter('under 15 km, hut, loop')).toEqual(enter(EXAMPLES[0]));
        expect(enter('round trip past Pilatus, at least T4, lifts')).toEqual({ from: ['Anywhere on the map', 'Lifts'], hike: ['Loop', 'A T4 part, up to T6'], way: ['Passes Pilatus'], stays: [] });
    });

    it('read a number in the units of the rider, and suggest what it can mean', () => {
        expect(labels('12')).toEqual(['Under 12 km', '10 km to 14 km', '12 km or more']);
        expect(labels('50')).toEqual(['Under 50 km', '40 km to 60 km', '50 km or more', 'Under 50 m climb', '40 m to 60 m climb', '50 m or more climb']);
        expect(labels('at least')).toEqual(['10 km or more', '600 m or more climb', 'A T3 part, up to T6']);
        settings.units = 'mi';
        onTestFinished(() => { settings.units = 'km'; });
        expect(labels('12')).toEqual(['Under 12 mi', '10 mi to 14 mi', '12 mi or more']);
        expect(enter('8-12, under 2000')).toMatchObject({ hike: ['8 mi to 12 mi', 'Under 2,000 ft climb'] });
        // A unit that the rider types wins, and a comma before three digits groups thousands.
        expect(enter('under 10 km')).toMatchObject({ hike: ['Under 6.2 mi'] });
        expect(enter('under 2,000 ft climb')).toMatchObject({ hike: ['Under 2,000 ft climb'] });
        settings.units = 'km';
        expect(enter('1,5 km, under 1,500 m climb')).toMatchObject({ hike: ['Under 1.5 km', 'Under 1,500 m climb'] });
    });

    it('read a number with no unit only before a phrase, a stop or the end', () => {
        expect(enter('12 loop')).toMatchObject({ hike: ['Loop', 'Under 12 km'] });
        expect(enter('under 4 hours, a 1 day hike, within 30 minutes')).toEqual({ from: [], hike: [], way: [], stays: [['4', undefined], ['hours', undefined], ['1', undefined], ['30', undefined], ['minutes', undefined]] });
    });

    it('give the opposite of a wish after "no", or keep the words with a note', () => {
        expect(enter('no forest, without big roads, no snow')).toMatchObject({ way: ['Free of snow', 'Far from big roads', 'Mostly open'], stays: [] });
        expect(enter('loop, no lake')).toEqual({ from: [], hike: ['Loop'], way: [], stays: [['no lake', 'Explore cannot leave out hikes with a lake.']] });
        expect(enter('snow')).toMatchObject({ way: [], stays: [['snow', undefined]] });
    });

    it('keep a word that they do not know, bike words and the values of a slot that is off, with a note', () => {
        expect(enter('sunny loop by gravel bike')).toMatchObject({ hike: ['Loop'], stays: [['sunny', undefined], ['gravel', 'Explore searches hikes only for now.'], ['bike', 'Explore searches hikes only for now.']] });
        expect(scan('hut', phraseTable(loaded, { way: 'Not for long trails' }))).toEqual([{ from: 0, to: 3, kind: 'unknown', note: 'Not for long trails: Hut with food.' }]);
    });

    it('ask for the start point with the car and a drive, which need one', () => {
        expect(scan('by car within 20 km', phraseTable(loaded, {})).map(part => part.kind === 'value' && part.value.here)).toEqual([true, true]);
    });
});
