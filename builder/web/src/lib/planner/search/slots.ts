/** The three slots of the planner search, "Find", "Where" and "When": the fields of a request that each slot holds, the
 * request that the slots give, their words, and how the request of an answer of the query model fills them. */
import { contract, kindLabel, pointLabel } from './presentation';
import type { Along, QueryDay, QueryPoint, QueryRequest, Quantity, SearchContext, Where } from './types';

export type SlotId = 'find' | 'where' | 'when';
/** One thing to find: a kind of place, with the cuisine of a kind of food; a place by its name; or a fact of the route,
 * with its least size. */
export type Find = { kind: string; cuisine?: string } | { name: string } | { fact: string; min?: Quantity };
export type Open = NonNullable<QueryRequest['open']>;
/** What a press on a place of the list does, by the request type of the change; with none, "Find" adds it as a stop. */
export type Act = 'route' | 'add_point' | 'end_day';
/** The values of the slots. "Find" holds `find` and `act`, "Where" holds `where` and `radius`. */
export interface Slots { find?: Find; act?: Act; where?: Where; radius?: Quantity; open?: Open }

/** The request of the slots, with the fields that its type takes; none without a thing to find. A named place takes only
 * the first place that it is near. A search area on the route goes as its stretch from km to km on the route of `plan`. */
export function requestOf({ find, where: area, radius, open }: Slots, plan?: Plan): QueryRequest | undefined {
    if (!find) return undefined;
    if ('name' in find) return { type: 'place', name: find.name, ...area?.near ? { near: area.near[0] } : {} };
    const stretch = plan && stretchOf(area, plan), where = stretch ? stretchWhere(stretch) : area;
    if ('fact' in find) return { type: 'stretches', what: find.fact, ...where ? { where } : {}, ...find.min ? { min: find.min } : {} };
    return { type: 'places', what: [find.kind], ...find.cuisine ? { cuisine: find.cuisine } : {}, ...where ? { where } : {}, ...radius ? { radius } : {}, ...open ? { open } : {} };
}

// ── The search area on the route ────────────────────────────────────────────

/** A stretch of the route, in km from its start. */
export type Stretch = [from: number, to: number];
/** The route and its days, in km. */
export type Plan = Pick<SearchContext['plan'], 'km' | 'days'>;

/** The search area of a stretch, at least a tenth of a km long. Its ends go out to a tenth of a km, so a place at an end
 * stays in it; the server limits the stretch to the route. */
export function stretchWhere([from, to]: Stretch): Where {
    const start = Math.floor(from * 10 + 1e-6) / 10, end = Math.max(Math.ceil(to * 10 - 1e-6) / 10, start + .1);
    return { along: { ref: 'km', from: { value: start, unit: 'km' }, to: { value: Math.round(end * 10) / 10, unit: 'km' } } };
}
/** How far ahead of the rider "Nearest to me" searches, in km. The owner has not chosen this length yet. */
const NEAREST_KM = 20;
/** The route ahead of the rider, from the place of the rider on it; the server ends it at the end of the route. */
export const NEAREST: Where = { along: { ref: 'here', from: { value: 0, unit: 'km' }, to: { value: NEAREST_KM, unit: 'km' } } };

/** A distance of the request in km; none for a time. */
const kmOf = (quantity: Quantity | undefined) => quantity?.unit === 'km' ? quantity.value : undefined;

/** The part of `base` that `along` names, in km from the start of the route; none for a time, a mark, or a distance from
 * the finish or the rider. An open end stays the end of `base`, as on the server: "after km 10" runs to the finish. */
function alongIn({ ref, at, from, to }: Along, base: Stretch): Stretch | undefined {
    if ((ref !== 'km' && ref !== 'start') || at) return undefined;
    const start = from ? kmOf(from) : base[0], end = to ? kmOf(to) : base[1];
    return start === undefined || end === undefined ? undefined : [start, end];
}

/** The km of a point that limits a stretch: a mark from the start, the start or the finish, or the end of a day for
 * "after" and its start for "before". None for a place, which only the server finds. */
function limitKm(point: QueryPoint, key: 'after' | 'before', total: number, days: Plan['days']): number | undefined {
    if ('along' in point) return point.along.ref === 'km' || point.along.ref === 'start' ? kmOf(point.along.at) : undefined;
    if ('plan' in point) return point.plan === 'start' ? 0 : total;
    if ('day' in point && typeof point.day === 'number' && !point.part) {
        const day = days.find(one => one.number === point.day);
        return day && (key === 'after' ? day.to : day.from);
    }
    return undefined;
}

/** The stretch that a search area on the route covers before its part, within the route: the whole route or a day of the
 * plan, narrowed by `along` and by a point "after" or "before" it, as the server reads it. None for an area near a place,
 * the rider or a point, for a limit that only the server can place, or for a stretch that the route does not reach. */
export function baseOf(where: Where | undefined, { km, days }: Plan): Stretch | undefined {
    const total = km.at(-1) ?? 0;
    if (!where || !total || where.near || where.anchor) return undefined;
    const { along, day, scope, before, after } = where;
    let base: Stretch | undefined;
    if (typeof day === 'number') {
        const found = days.find(one => one.number === day && !one.rest);
        base = found && [found.from, found.to];
    } else if (day === undefined && (scope === 'route' || along || before || after)) base = [0, total];
    if (base && along) base = alongIn(along, base);
    for (const [key, point] of [['after', after], ['before', before]] as const) {
        if (!base || !point) continue;
        const at = limitKm(point, key, total, days);
        base = at === undefined ? undefined : key === 'after' ? [Math.max(base[0], at), base[1]] : [base[0], Math.min(base[1], at)];
    }
    const [from, to] = [Math.max(0, base?.[0] ?? 0), Math.min(total, base?.[1] ?? 0)];
    return base && from < to ? [from, to] : undefined;
}

/** A part of a stretch: at an end, `max(10 km, 20 %)` of it; in the middle, its middle 40 %. */
export function partOf([from, to]: Stretch, part?: string): Stretch {
    const length = to - from, edge = Math.min(length, Math.max(10, length * .2));
    if (part === 'start') return [from, from + edge];
    if (part === 'end') return [to - edge, to];
    return part === 'middle' ? [from + length * .3, from + length * .7] : [from, to];
}

/** The stretch of a search area on the route, after its part. */
export function stretchOf(where: Where | undefined, plan: Plan): Stretch | undefined {
    const base = baseOf(where, plan);
    return base && partOf(base, where!.part);
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

/** The action of a request that changes the plan, with the request of the places that it lists: a kind, in the area of
 * the request, or a name. None when the slots cannot hold the request, such as a route with a start or a day count, a
 * stop every 30 km, or a day end at a km mark. */
function actionOf({ type, to, point, at, day, where, ...rest }: QueryRequest): { act: Act; listed: QueryRequest } | undefined {
    const [act, place, area]: [Act?, QueryPoint?, Where?] =
        type === 'route' && !rest.from && !rest.via && !rest.bike && !rest.goal && !rest.days && !rest.per_day ? [type, to]
        : type === 'add_point' && !rest.every && rest.kind !== 'pass' ? [type, point, where]
        // A day end is near the end of its day.
        : type === 'end_day' && day !== undefined && day !== 'every' ? [type, at, { day, part: 'end' }] : [];
    if (act && place && 'kind' in place) return { act, listed: { type: 'places', what: [place.kind], ...area ? { where: area } : {} } };
    if (act && place && 'name' in place) return { act, listed: { type: 'place', name: place.name } };
}

/** The values of the slots that a request holds, and the kinds that one thing to find left out. A request that changes
 * the plan holds its action and the places that it lists, or no value. */
export function slotsOf(request: QueryRequest): { slots: Slots; left: string[] } {
    const action = actionOf(request), listed = action && slotsOf(action.listed);
    if (listed) return { ...listed, slots: { ...listed.slots, act: action!.act } };
    const { type, what, cuisine, name, near, where: area, radius, open, min } = request;
    // A part goes with an area on the route: the server reads none near a place, the rider or a point.
    const { part: _, ...loose } = area ?? {}, where = area?.part && (area.near || area.anchor || area.scope === 'here') ? loose : area;
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
    const what = found.find ? found : slots;
    return { find: what.find, act: what.act, where: area.where, radius: area.radius, open: found.open ?? slots.open };
}

/** The slots with action `act`, or none for "Find". An action is on a place, so a fact of the route leaves "Find". */
export const withAct = (slots: Slots, act: Act | undefined): Slots =>
    ({ ...slots, act, ...act && slots.find && 'fact' in slots.find ? { find: undefined } : {} });

/** The slots with the values of `slot` at their defaults. */
export function resetSlot(slots: Slots, slot: SlotId): Slots {
    return slot === 'find' ? { ...slots, find: undefined, act: undefined } : slot === 'where' ? { ...slots, where: undefined, radius: undefined } : { ...slots, open: undefined };
}

/** The same value, whatever the order of the keys of its objects. */
export const sameValue = (a: unknown, b: unknown) => {
    const sorted = (value: unknown) => JSON.stringify(value, (_, v) => v && typeof v === 'object' && !Array.isArray(v) ? Object.fromEntries(Object.entries(v).sort()) : v);
    return sorted(a) === sorted(b);
};

// ── Words ───────────────────────────────────────────────────────────────────

/** The label of "Find" with an action, the button of the action on a place, and its icon. */
export const ACTS: Record<Act, { label: string; button: string; icon: string }> = {
    route: { label: 'Route to', button: 'Route here', icon: 'route' },
    add_point: { label: 'Add a stop at', button: 'Add as a stop', icon: 'plus' },
    end_day: { label: 'End the day at', button: 'End the day here', icon: 'moon' },
};

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
        where.along && (sameValue(where, NEAREST) ? 'Nearest to me' : where.part ? `${capital(where.part)} of ${pointLabel({ along: where.along })}` : pointLabel({ along: where.along })),
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
