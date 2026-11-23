import { describe, expect, it } from 'vitest';
import { scan, suggest, typing, type Table, type Value } from './read';

const value = (label: string): Value => ({ label, slot: 'one', icon: 'x' });
const [LIFT, UP, HUT, FALL, QUIET] = ['Lifts', 'Lift up', 'Hut', 'Waterfall', 'Far from big roads'].map(value);
const TABLE: Table<Value> = {
    phrases: [{ words: 'lift', value: LIFT }, { words: 'lift up', value: UP }, { words: 'hut', value: HUT }, { words: 'waterfall', value: FALL },
        { words: 'far from big roads', value: QUIET }, { words: 'bike', value: 'Hikes only.' }],
    patterns: [{ match: /^(?:(under)\s+)?(\d+)\s*(km)?(?![\p{L}\d])/u, values: ([, under, count]) => [value(`Under ${count}`), ...under ? [] : [value(`${count} or more`)]] }],
    starts: [{ words: ['under', 'at least'], values: [value('Under 10')] }],
    units: ['km'],
    glue: ['with', 'a'],
    not: ['no'],
};
/** Each part as its kind and its words. */
const parts = (text: string) => scan(text, TABLE).map(part => [part.kind, text.slice(part.from, part.to)]);
const labels = (fragment: string) => suggest(fragment, TABLE).map(one => one.label);

describe('the reader', () => {
    it('marks the longest phrase, glue words and unknown words', () => {
        expect(parts('lift up with a hut')).toEqual([['value', 'lift up'], ['glue', 'with'], ['glue', 'a'], ['value', 'hut']]);
        expect(scan('lift up', TABLE)[0]).toMatchObject({ value: UP });
        expect(parts('sunny lift, under 12 km')).toEqual([['unknown', 'sunny'], ['value', 'lift'], ['glue', ','], ['value', 'under 12 km']]);
        // A phrase with a note stays unknown and says why.
        expect(scan('bike', TABLE)).toEqual([{ from: 0, to: 4, kind: 'unknown', note: 'Hikes only.' }]);
    });

    it('reads a plural, capitals and accents', () => {
        expect(scan('Hüts', TABLE)).toEqual([{ from: 0, to: 4, kind: 'value', value: HUT }]);
    });

    it('finds the words that the rider still types', () => {
        const start = (text: string) => text.slice(typing(text, TABLE));
        expect(start('lift with a hu')).toBe('hu');
        // More words when they begin a longer phrase or a quantity, with its word before the number.
        expect(start('hut far from b')).toBe('far from b');
        expect(start('hut under 12 ')).toBe('under 12 ');
        expect(start('hut at ')).toBe('at ');
        // After a space, a whole phrase and a number with its unit are done.
        expect(start('lift ')).toBe('');
        expect(start('under 12 km ')).toBe('');
    });

    it('suggests by the first letters, a quantity, and with one typing error', () => {
        expect(labels('li')).toEqual(['Lifts', 'Lift up']);
        expect(labels('12')).toEqual(['Under 12', '12 or more']);
        expect(labels('at lea')).toEqual(['Under 10']);
        expect(labels('lidt')).toEqual(['Lifts', 'Lift up']);
        expect(labels('eaterfall')).toEqual(['Waterfall']);
        // Three letters allow no error, and a whole word keeps its own meaning.
        expect(labels('hxt')).toEqual([]);
        expect(labels('with')).toEqual([]);
        expect(labels('lift')).toEqual(['Lifts']);
    });
});
