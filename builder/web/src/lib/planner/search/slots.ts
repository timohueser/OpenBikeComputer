/** The three slots of the planner search, "Find", "Where" and "When": the fields of a request that each slot holds, the
 * request that the slots give, their words, and how the request of an answer of the query model fills them. */
import { contract, kindLabel, pointLabel } from './presentation';
import type { QueryDay, QueryRequest, Quantity, Where } from './types';

export type SlotId = 'find' | 'where' | 'when';
/** One thing to find: a kind of place, with the cuisine of a kind of food; a place by its name; or a fact of the route,
 * with its least size. */
export type Find = { kind: string; cuisine?: string } | { name: string } | { fact: string; min?: Quantity };
export type Open = NonNullable<QueryRequest['open']>;
/** The values of the slots. "Where" holds `where` and `radius`. */
export interface Slots { find?: Find; where?: Where; radius?: Quantity; open?: Open }

/** The request of the slots, with the fields that its type takes; none without a thing to find. A named place takes only
 * the first place that it is near. */
export function requestOf({ find, where, radius, open }: Slots): QueryRequest | undefined {
    if (!find) return undefined;
    if ('name' in find) return { type: 'place', name: find.name, ...where?.near ? { near: where.near[0] } : {} };
    if ('fact' in find) return { type: 'stretches', what: find.fact, ...where ? { where } : {}, ...find.min ? { min: find.min } : {} };
    return { type: 'places', what: [find.kind], ...find.cuisine ? { cuisine: find.cuisine } : {}, ...where ? { where } : {}, ...radius ? { radius } : {}, ...open ? { open } : {} };
}

/** A kind and the kinds over it, the nearest first. */
function lineage(kind: string): string[] {
    const kinds: Record<string, { parent: string | null }> = contract.kinds, line = [kind];
    for (let parent = kinds[kind]?.parent; parent; parent = kinds[parent]?.parent) line.push(parent);
    return line;
}
/** The top kind of a kind, such as `sleep` for `hotel`. */
export const topKind = (kind: string) => lineage(kind).at(-1)!;
/** The kinds under a kind, at any depth, in the order of the contract. */
export const kindsUnder = (kind: string) => Object.keys(contract.kinds).filter(other => other !== kind && lineage(other).includes(kind));

/** One kind for several: the nearest kind that holds them all, else the first. `left` are the kinds that it leaves out. */
export function oneKind(kinds: string[]): { kind: string; left: string[] } {
    const shared = lineage(kinds[0]).find(kind => kinds.every(other => lineage(other).includes(kind)));
    return shared ? { kind: shared, left: [] } : { kind: kinds[0], left: kinds.slice(1) };
}

/** The values of the slots that a request holds, and the kinds that one thing to find left out. A request that changes
 * the plan holds none. */
export function slotsOf({ type, what, cuisine, name, near, where, radius, open, min }: QueryRequest): { slots: Slots; left: string[] } {
    if (type === 'places' && what?.length) {
        const { kind, left } = oneKind([what].flat());
        return { slots: { find: { kind, ...cuisine ? { cuisine } : {} }, where, radius, open }, left };
    }
    if (type === 'place' && name) return { slots: { find: { name }, ...near ? { where: { near: [near] } } : {} }, left: [] };
    if (type === 'stretches' && typeof what === 'string') return { slots: { find: { fact: what, ...min ? { min } : {} }, where }, left: [] };
    return { slots: {}, left: [] };
}

/** The slots after an answer of the query model: each slot that the answer holds a value of takes it, and the others keep
 * theirs. A named place keeps no search area: it is searched near the place that its answer names, else in the map view. */
export function fill(slots: Slots, found: Slots): Slots {
    const area = found.where || found.radius || (found.find && 'name' in found.find) ? found : slots;
    return { find: found.find ?? slots.find, where: area.where, radius: area.radius, open: found.open ?? slots.open };
}

/** The slots with the values of `slot` at their defaults. */
export function resetSlot(slots: Slots, slot: SlotId): Slots {
    return slot === 'find' ? { ...slots, find: undefined } : slot === 'where' ? { ...slots, where: undefined, radius: undefined } : { ...slots, open: undefined };
}

/** The same value, whatever the order of the keys of its objects. */
export const sameValue = (a: unknown, b: unknown) => {
    const sorted = (value: unknown) => JSON.stringify(value, (_, v) => v && typeof v === 'object' && !Array.isArray(v) ? Object.fromEntries(Object.entries(v).sort()) : v);
    return sorted(a) === sorted(b);
};

// ── Words ───────────────────────────────────────────────────────────────────

const capital = (text: string) => text.charAt(0).toUpperCase() + text.slice(1);
/** The name of a kind of place, such as "Drinking water". */
export const kindWord = (kind: string) => kind === 'atm' ? 'ATM' : capital(kindLabel(kind));

const FACTS: Record<string, [word: string, icon: string]> = {
    climb: ['Climbs', 'ascent'], descent: ['Descents', 'descent'], steep: ['Steep parts', 'warning'], unpaved: ['Unpaved', 'gravel'],
    unknown_surface: ['Unknown surface', 'info'], pushing: ['Pushing', 'hiking'], closure: ['Closures', 'close'],
};
/** The name of a fact of the route, such as "Climbs" or "Longest stretch without drinking water". */
export const factWord = (fact: string) => fact.startsWith('gap:') ? `Longest stretch without ${kindLabel(fact.slice(4))}` : FACTS[fact]?.[0] ?? capital(kindLabel(fact));
export const factIcon = (fact: string) => fact.startsWith('gap:') ? 'water' : FACTS[fact]?.[1] ?? 'route';

const dayWord = (day: QueryDay) => typeof day === 'number' ? `Day ${day}` : day === 'every' ? 'every day' : day;
const WEEKDAYS: Record<string, string> = { mon: 'Monday', tue: 'Tuesday', wed: 'Wednesday', thu: 'Thursday', fri: 'Friday', sat: 'Saturday', sun: 'Sunday' };
export const weekdayWord = (day: string) => WEEKDAYS[day] ?? day;

/** The words of a search area, such as "Middle of Day 1" or "Near Freiburg". */
export function whereWords(where: Where | undefined): string[] {
    if (!where) return [];
    const words = [
        where.anchor && 'Near the selected point',
        where.day !== undefined && capital(where.part ? `${where.part} of ${dayWord(where.day)}` : dayWord(where.day)),
        where.scope === 'route' && (where.part && where.day === undefined ? `${capital(where.part)} of the route` : 'Whole route'),
        where.scope === 'here' && 'Near me',
        where.scope === 'view' && 'In this map view',
        where.near && `${where.near.length > 1 ? 'Between' : 'Near'} ${where.near.map(pointLabel).join(' and ')}`,
        where.along && pointLabel({ along: where.along }),
        where.before && `Before ${pointLabel(where.before)}`,
        where.after && `After ${pointLabel(where.after)}`,
    ];
    return words.filter((word): word is string => !!word);
}

/** The words of an opening time, such as "Open now" or "Open on Sunday". */
export function openWords(open: Open | undefined): string[] {
    if (!open) return [];
    if (open.now) return ['Open now'];
    if (open.weekday) return [`Open on ${weekdayWord(open.weekday)}`];
    const day = open.day ?? 'today';
    return [typeof day === 'number' ? `Open on Day ${day}` : `Open ${dayWord(day)}`];
}

function findWords(find: Find | undefined): string[] {
    if (!find) return [];
    if ('name' in find) return [find.name];
    if ('fact' in find) return [factWord(find.fact), ...find.min ? [`At least ${find.min.value} ${find.min.unit}`] : []];
    return [kindWord(find.kind), ...find.cuisine ? [capital(kindLabel(find.cuisine))] : []];
}

/** The words of each slot, in a fixed order. A slot with no words shows its default. */
export function slotWords(slots: Slots): Record<SlotId, string[]> {
    const radius = slots.radius ? [`Within ${slots.radius.value} ${slots.radius.unit}`] : [];
    return { find: findWords(slots.find), where: [...whereWords(slots.where), ...radius], when: openWords(slots.open) };
}
