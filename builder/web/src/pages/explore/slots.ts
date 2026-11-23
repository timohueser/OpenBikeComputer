/** The three slots of the search bar of Explore: the fields of `Query` that each slot owns, the words that it shows, and
 * the query that the page searches while a slot is off. */
import type { FactsPlace } from '../../lib/planner/signed-routes';
import { formatHeight } from '../../lib/settings/format';
import { rangeText } from '../routes/filter';
import { defaultQuery, driveOf, GRADE_TOP, liftsOff, SHAPES, wishesOf, type Query, type Shape } from './search';
import { placeName, SHAPE_WORDS, shortDistance, walked, wishLabel } from './words';

export type SlotId = 'from' | 'hike' | 'way';

/** The fields that each slot owns: "Reset" of the slot gives them their value in `defaultQuery`. */
export const SLOT_FIELDS = {
    from: ['start', 'pin', 'car', 'drive', 'walk', 'lift'],
    hike: ['shapes', 'length', 'climb', 'grade', 'ungraded'],
    way: ['wishes', 'date'],
} as const satisfies Record<SlotId, readonly (keyof Query)[]>;

/** The query with the fields of `slot` at their default. Lifts that go off leave a shape that counts without them. */
export function resetSlot(query: Query, slot: SlotId, today: string): Query {
    const base = defaultQuery(today);
    const next: Query = { ...query, ...Object.fromEntries(SLOT_FIELDS[slot].map(key => [key, base[key]])) };
    return next.lift ? next : { ...next, shapes: liftsOff(next.shapes) };
}

/** Whether a field of the slot differs from its default: its "Reset" then has work, and the cross shows. A slot can hold a
 * value that it shows no word for, such as the car with no start point. */
export function slotSet(query: Query, slot: SlotId, today: string): boolean {
    const base = defaultQuery(today);
    return SLOT_FIELDS[slot].some(key => JSON.stringify(query[key]) !== JSON.stringify(base[key]));
}

/** What the page cannot search: "From" and "On the way" with no facts files or on the long trails tab, and the shape,
 * the length, the climb and the lower grade on the long trails tab. */
export interface Off { from: boolean; way: boolean; trails: boolean }

/** The query that the page searches and shows: each value that is off has its default, so a query from the address or
 * the history cannot filter with it. The page keeps the query itself, so the value comes back where it is on again. */
export function effectiveQuery(query: Query, off: Off, today: string): Query {
    let next = query;
    if (off.from) next = resetSlot(next, 'from', today);
    if (off.way) next = resetSlot(next, 'way', today);
    if (off.trails) next = { ...next, shapes: [...SHAPES], length: undefined, climb: undefined, grade: [0, next.grade[1]], ungraded: true };
    return next;
}

/** "12 Dec": a day `YYYY-MM-DD`. */
const dayText = (date: string) => new Date(`${date}T00:00:00Z`).toLocaleDateString('en-GB', { day: 'numeric', month: 'short', timeZone: 'UTC' });

function fromWords(query: Query, place: (id: string) => FactsPlace | undefined): string[] {
    if (!query.start && !query.pin && !query.lift) return [];
    const words = [query.pin ? placeName(place(query.pin)) : query.start ? 'Start point' : 'Anywhere on the map'];
    if (query.pin) words.push(`Walk up to ${walked(query.walk)}`);
    else if (query.start) words.push(query.car ? `By car, up to ${driveOf(query)} km` : `On foot, up to ${walked(query.walk)}`);
    if (query.lift) words.push(query.lift === 'always' ? 'Always a lift' : 'Lifts');
    return words;
}

/** The accepted shapes of those that count, or none when all of them are accepted: lift shapes count while lifts are on. */
export function shapesSet(query: Query): Shape[] {
    const counted = SHAPES.filter(shape => query.lift || (shape !== 'liftDown' && shape !== 'liftUp'));
    const shapes = counted.filter(shape => query.shapes.includes(shape));
    return shapes.length < counted.length ? shapes : [];
}

/** "A T2 part, up to T4", "Up to T3", or none for any grade. */
export function gradeText({ grade: [low, high] }: Pick<Query, 'grade'>): string | undefined {
    return low > 0 ? `A T${low + 1} part, up to T${high + 1}` : high < GRADE_TOP ? `Up to T${high + 1}` : undefined;
}

function hikeWords(query: Query): string[] {
    const words = shapesSet(query).map(shape => SHAPE_WORDS[shape].tag);
    if (query.length) words.push(rangeText(query.length, shortDistance));
    if (query.climb) words.push(`${rangeText(query.climb, formatHeight)} climb`);
    const grade = gradeText(query);
    return grade ? [...words, grade] : words;
}

function wayWords(query: Query, place: (id: string) => FactsPlace | undefined, today: string): string[] {
    return wishesOf(query).map(wish => {
        const label = wishLabel(wish, place, true) + (wish === 'snow' && query.date !== today ? ` on ${dayText(query.date)}` : '');
        return query.wishes[wish] === 'nice' ? `${label} if possible` : label;
    });
}

/** The words of each slot, in a fixed order. A slot with no words shows its default. */
export function slotWords(query: Query, place: (id: string) => FactsPlace | undefined, today: string): Record<SlotId, string[]> {
    return { from: fromWords(query, place), hike: hikeWords(query), way: wayWords(query, place, today) };
}
