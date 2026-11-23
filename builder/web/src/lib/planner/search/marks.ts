/** The English words of the planner search that the text field marks and suggests while the rider types: the actions on a
 * place, kinds of places, facts of the route, days and their parts, search areas and opening times. The lexicon builder of the query model
 * writes them to `marks.json`. Each gives the value of one slot; the query model reads a sentence at Enter. */
import marks from '../../../../../../planner/search/query/marks.json' with { type: 'json' };
import { fold, scan, type Table, type Value } from '../../ui/search-bar/read';
import { placeCategories } from '../poi-kinds';
import { category } from './presentation';
import { ACTS, factIcon, factWord, kindWord, NEAREST, openWords, whereWords, withAct, type Act, type SlotId, type Slots } from './slots';
import type { QueryDay, Where } from './types';

/** A value of some words: a change of the slots. `kind` is the kind of a search area or an opening time. */
export interface Mark extends Value { slot: SlotId; kind?: 'day' | 'part' | 'scope' | 'open'; set: (slots: Slots) => Slots }

export const kindMark = (kind: string): Mark =>
    ({ label: kindWord(kind), slot: 'find', icon: 'search', path: placeCategories[category(kind)].icon, set: slots => ({ ...slots, find: { kind } }) });
export const factMark = (fact: string): Mark => ({ label: factWord(fact), slot: 'find', icon: factIcon(fact), set: slots => ({ ...slots, find: { fact } }) });
const actMark = (act: Act): Mark => ({ label: ACTS[act].label, slot: 'find', icon: ACTS[act].icon, set: slots => withAct(slots, act) });

/** A search area from a day, a part or a scope, which the other words of a sentence complete: "middle" and "day 1" give
 * the middle of day 1, and a part with no day is a part of the route. */
function area(kind: 'day' | 'part' | 'scope', label: string, icon: string, place: (where: Where | undefined) => Where): Mark {
    return { label, slot: 'where', kind, icon, set: slots => ({ ...slots, where: place(slots.where) }) };
}
export const dayMark = (day: QueryDay, part?: string) => area('day', whereWords({ day, part })[0], 'calendar', where => {
    const kept = part ?? where?.part;
    return kept ? { day, part: kept } : { day };
});
const partMark = (part: string) =>
    area('part', part.charAt(0).toUpperCase() + part.slice(1), 'route', where => where?.day !== undefined ? { day: where.day, part } : { scope: 'route', part });
export const scopeMark = (scope: string) => area('scope', whereWords({ scope })[0], scope === 'here' ? 'locate' : scope === 'view' ? 'fit' : 'route',
    where => scope === 'route' && where?.part && where.day === undefined ? { scope, part: where.part } : { scope });

const when = (open: NonNullable<Slots['open']>): Mark =>
    ({ label: openWords(open)[0], slot: 'when', kind: 'open', icon: open.now ? 'clock' : 'calendar', set: slots => ({ ...slots, open }) });
export const OPEN_NOW = when({ now: true });
/** "The next supermarket": the nearest one ahead on the route. */
const NEAREST_MARK = area('scope', whereWords(NEAREST)[0], 'locate', () => NEAREST);

function table(): Table<Mark> {
    const phrases: Table<Mark>['phrases'] = [];
    const add = (terms: string[], value: Mark) => phrases.push(...terms.map(words => ({ words, value })));
    for (const [act, terms] of Object.entries(marks.action)) add(terms, actMark(act as Act));
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
    add(marks.now.now, OPEN_NOW);
    for (const [weekday, terms] of Object.entries(marks.weekday)) add(terms.flatMap(term => [term, `open ${term}`, `open on ${term}`]), when({ weekday }));
    const nouns = marks.day_noun.join('|');
    return {
        phrases, starts: [], units: [], not: [], glue: marks.glue,
        patterns: [
            { match: new RegExp(`^(?:${nouns})\\s+(\\d{1,2})(?![\\p{L}\\d])`, 'u'), values: ([, n]) => [dayMark(Number(n))] },
            // "The next 20 km" is a stretch, which the query model reads.
            { match: new RegExp(`^(?:${marks.nearest.join('|')})(?![\\p{L}\\d]|\\s*\\d)`, 'u'), values: () => [NEAREST_MARK] },
        ],
    };
}
/** The words of the planner search for the reader of the text field. */
export const PLANNER_WORDS = table();

/** A text says "the next" or "the nearest" of a place: the query model reads these words as no search area. */
export const saysNearest = (text: string) => scan(text, PLANNER_WORDS).some(part => part.kind === 'value' && part.value === NEAREST_MARK);

const STOP = new Set(marks.stop);
/** Two values that one search area or opening time cannot hold together: two of one kind, or a scope and a day. A part
 * goes with a day or a scope. */
const clash = (a: Mark, b: Mark) => a.kind === b.kind || (a.kind === 'scope' && b.kind === 'day') || (a.kind === 'day' && b.kind === 'scope');
/** The marks of a sentence of only search areas and opening times whose other words carry no meaning, which the page can
 * read with no model; undefined for any other sentence, such as one with "after" or two days. */
export function areasAndTimes(text: string): Mark[] | undefined {
    const found: Mark[] = [];
    for (const part of scan(text, PLANNER_WORDS)) {
        const words = fold(text.slice(part.from, part.to));
        if (part.kind === 'value' && part.value.slot !== 'find' && !found.some(mark => clash(mark, part.value))) found.push(part.value);
        else if (part.kind !== 'glue' || !(STOP.has(words) || /^[,;.!?]$/.test(words))) return undefined;
    }
    return found.length ? found : undefined;
}

/** The values that riders often add next: a thing to find, then a day, the opening time and a search area. */
export function oftenNext(slots: Slots, days: number): Mark[] {
    const { find, where, radius, open } = slots;
    if (!find) return [kindMark('water'), kindMark('food'), kindMark('sleep'), factMark('climb')];
    if ('name' in find) return [];
    const free = !where && !radius;
    return [free && days > 1 && dayMark(1), 'kind' in find && !open && OPEN_NOW, free && scopeMark('here'), free && scopeMark('route')]
        .filter((mark): mark is Mark => !!mark);
}
