/** The English words of the planner search that the text field marks and suggests while the rider types: kinds of places,
 * facts of the route, days and their parts, search areas and opening times. The lexicon builder of the query model
 * writes them to `marks.json`. Each gives the value of one slot; the query model reads a sentence at Enter. */
import marks from '../../../../../../planner/search/query/marks.json' with { type: 'json' };
import type { Table, Value } from '../../ui/search-bar/read';
import { placeCategories } from '../poi-kinds';
import { category } from './presentation';
import { factIcon, factWord, kindWord, openWords, whereWords, type SlotId, type Slots } from './slots';
import type { QueryDay, Where } from './types';

/** A value of some words: a change of the slots. */
export interface Mark extends Value { slot: SlotId; set: (slots: Slots) => Slots }

export const kindMark = (kind: string): Mark =>
    ({ label: kindWord(kind), slot: 'find', icon: 'search', path: placeCategories[category(kind)].icon, set: slots => ({ ...slots, find: { kind } }) });
export const factMark = (fact: string): Mark => ({ label: factWord(fact), slot: 'find', icon: factIcon(fact), set: slots => ({ ...slots, find: { fact } }) });

/** A search area from a day, a part or a scope, which the other words of a sentence complete: "middle" and "day 1" give
 * the middle of day 1, and a part with no day is a part of the route. */
function area(label: string, icon: string, place: (where: Where | undefined) => Where): Mark {
    return { label, slot: 'where', icon, set: slots => ({ ...slots, where: place(slots.where) }) };
}
export const dayMark = (day: QueryDay, part?: string) => area(whereWords({ day, part })[0], 'calendar', where => {
    const kept = part ?? where?.part;
    return kept ? { day, part: kept } : { day };
});
const partMark = (part: string) =>
    area(part.charAt(0).toUpperCase() + part.slice(1), 'route', where => where?.day !== undefined ? { day: where.day, part } : { scope: 'route', part });
export const scopeMark = (scope: string) => area(whereWords({ scope })[0], scope === 'here' ? 'locate' : scope === 'view' ? 'fit' : 'route',
    where => scope === 'route' && where?.part && where.day === undefined ? { scope, part: where.part } : { scope });

const when = (open: NonNullable<Slots['open']>): Mark =>
    ({ label: openWords(open)[0], slot: 'when', icon: open.now ? 'clock' : 'calendar', set: slots => ({ ...slots, open }) });
export const OPEN_NOW = when({ now: true });

function table(): Table<Mark> {
    const phrases: Table<Mark>['phrases'] = [];
    const add = (terms: string[], value: Mark) => phrases.push(...terms.map(words => ({ words, value })));
    // A word of two values takes the first: "hills" are climbs, not summits.
    for (const [fact, terms] of Object.entries(marks.stretch)) add(terms, factMark(fact));
    for (const [kind, terms] of Object.entries(marks.kind)) add(terms, kindMark(kind));
    for (const [part, terms] of Object.entries(marks.part)) add(terms, partMark(part));
    for (const [key, terms] of Object.entries(marks.day)) {
        const [day, part] = key.split(' ');
        add(terms, dayMark(day as QueryDay, part));
    }
    for (const [n, terms] of Object.entries(marks.ordinal)) add(terms.flatMap(term => marks.day_noun.map(noun => `${term} ${noun}`)), dayMark(Number(n)));
    for (const [n, terms] of Object.entries(marks.number)) add(terms.flatMap(term => marks.day_noun.map(noun => `${noun} ${term}`)), dayMark(Number(n)));
    for (const [scope, terms] of Object.entries(marks.scope)) add(terms, scopeMark(scope));
    add(marks.open, OPEN_NOW);
    for (const [weekday, terms] of Object.entries(marks.weekday)) add(terms.flatMap(term => [term, `open ${term}`, `open on ${term}`]), when({ weekday }));
    const nouns = marks.day_noun.join('|');
    return {
        phrases, starts: [], units: [], not: [], glue: marks.glue,
        patterns: [{ match: new RegExp(`^(?:${nouns})\\s+(\\d{1,2})(?![\\p{L}\\d])`, 'u'), values: ([, n]) => [dayMark(Number(n))] }],
    };
}
/** The words of the planner search for the reader of the text field. */
export const PLANNER_WORDS = table();

/** The values that riders often add next: a thing to find, then a day, the opening time and a search area. */
export function oftenNext(slots: Slots, days: number): Mark[] {
    const { find, where, radius, open } = slots;
    if (!find) return [kindMark('water'), kindMark('food'), kindMark('sleep'), factMark('climb')];
    if ('name' in find) return [];
    const free = !where && !radius;
    return [free && days > 1 && dayMark(1), 'kind' in find && !open && OPEN_NOW, free && scopeMark('here'), free && scopeMark('route')]
        .filter((mark): mark is Mark => !!mark);
}
