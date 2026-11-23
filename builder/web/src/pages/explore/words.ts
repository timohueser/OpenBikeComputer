/** The words of the day-hike search: the names of the wishes, shapes, grades and orders, the numbers of a row, how a row
 * is reached and the changes that help an empty list. */
import type { CatalogFacts, FactsPlace } from '../../lib/planner/signed-routes';
import { distanceIn, formatDistance, formatHeight, formatHours } from '../../lib/settings/format';
import { settings } from '../../lib/settings/settings.svelte';
import { rangeText } from '../routes/filter';
import { WISHES, type Query, type Row, type Shape, type Sort, type Suggestion, type Way, type Wish } from './search';

type Place = (id: string) => FactsPlace | undefined;

/** The label of each wish, a short one for a row and the card, and its icon. */
const WISH_WORDS: Record<typeof WISHES[number], { label: string; short?: string; icon: string }> = {
    peak: { label: 'Summit', icon: 'mountain' },
    lake: { label: 'Lake', icon: 'lake' },
    hut: { label: 'Hut with food', icon: 'hut' },
    waterfall: { label: 'Waterfall', icon: 'waterfall' },
    official: { label: 'Official route', icon: 'flag' },
    snow: { label: 'Usually free of snow', short: 'Free of snow', icon: 'snow' },
    quiet: { label: 'Far from big roads', icon: 'quiet' },
    forest: { label: 'Mostly forest', icon: 'tree' },
    open: { label: 'Mostly open', icon: 'meadow' },
    water: { label: 'Along water', icon: 'water' },
};
/** The wishes of each group of the filter column. */
export const WAY_WISHES = ['peak', 'lake', 'hut', 'waterfall'] as const;
export const ROUTE_WISHES = ['official', 'snow', 'quiet', 'forest', 'open', 'water'] as const;

/** A place with no name has the word of its kind. */
const KIND_WORDS: Record<FactsPlace['kind'], string> = { parking: 'Car park', lift: 'Lift', peak: 'Summit', hut: 'Hut', lake: 'Lake', waterfall: 'Waterfall' };
const KIND_ICONS: Record<FactsPlace['kind'], string> = { parking: 'car', lift: 'lift', peak: 'mountain', hut: 'hut', lake: 'lake', waterfall: 'waterfall' };
export const placeName = (place: FactsPlace | undefined) => place?.name ?? KIND_WORDS[place?.kind ?? 'peak'];

/** "Summit", "Passes Pilatus"; `short` for a row and the card. */
export function wishLabel(wish: Wish, place: Place, short = false): string {
    if (wish.startsWith('via:')) return `Passes ${placeName(place(wish.slice(4)))}`;
    const words = WISH_WORDS[wish as typeof WISHES[number]];
    return (short && words.short) || words.label;
}

export const wishIcon = (wish: Wish, place: Place) =>
    wish.startsWith('via:') ? KIND_ICONS[place(wish.slice(4))?.kind ?? 'peak'] : WISH_WORDS[wish as typeof WISHES[number]].icon;

/** The tile of a shape, and the word of its row. */
export const SHAPE_WORDS: Record<Shape, { label: string; tag: string }> = {
    loop: { label: 'Loop', tag: 'Loop' },
    back: { label: 'There and back', tag: 'There and back' },
    liftDown: { label: 'Hike up, lift down', tag: 'Lift down' },
    liftUp: { label: 'Lift up, hike down', tag: 'Lift up' },
};

/** The orders of "Sort by". */
export const SORT_WORDS: Record<Sort, string> = {
    match: 'Best match', name: 'Name', shape: 'Shape', length: 'Length', climb: 'Climb', time: 'Time', grade: 'Grade', access: 'Getting there',
};

/** The numbers of a row in its shape: the length, the climb, the descent of a lift shape, the time and the grade. */
export function hikeNumbers(row: Row): string[] {
    const lift = row.shape === 'liftDown' || row.shape === 'liftUp', hardest = row.record.hardest;
    return [formatDistance(row.length_m), `${formatHeight(row.ascent_m)} up`, ...lift ? [`${formatHeight(row.descent_m)} down`] : [],
        ...row.moving_s === undefined ? [] : [formatHours(row.moving_s)], ...hardest === undefined ? [] : [`T${hardest + 1}`]];
}

/** The name and the description of each hiking grade, T1 to T6. */
export const GRADES: [name: string, about: string][] = [
    ['Good path', 'Flat or a gentle slope.'], ['Mountain path', 'Clear, with some steep parts.'],
    ['Rocky path', 'A chain in places, hands for balance.'], ['Alpine', 'The path is missing in places, hands needed.'],
    ['Hard alpine', 'Often no path, easy climbing.'], ['Climbing', 'Mostly no path.'],
];

/** The length and the climb that the scales of their sliders reach: most day hikes are below them. */
export const LENGTH_TOP_M = 30_000, CLIMB_TOP_M = 1500;

/** "900 m", "1 km", "12.5 km": a distance that the rider set, with no needless decimal. */
export const shortDistance = (metres: number) => settings.units === 'km' && metres < 1000 ? `${Math.round(metres)}\u00a0m`
    : `${distanceIn(metres).toLocaleString('en', { maximumFractionDigits: 1 })}\u00a0${settings.units}`;
/** The walk of a way shows from this distance; a shorter one is "the route starts there". */
const SHOWN_WALK_M = 150;
export const walked = (metres: number) => shortDistance(Math.max(100, Math.round(metres / 100) * 100));

/** One step of the way to a row: an icon, or the car park mark, and a few words. */
export type Step = { icon?: string; park?: true; text: string };

/** The place id of the car park that a row with no start point names: a large one within 300 m of the route, else the
 * nearest. */
export function parkAt(facts: CatalogFacts | undefined, place: Place): string | undefined {
    const near = (facts?.parking ?? []).filter(([id, metres]) => metres <= 300 && place(id)).map(([id]) => id);
    return near.find(id => place(id)!.size === 'large') ?? near[0];
}
/** The place id of the lift whose top station is within 300 m of the route. */
export const liftAt = (facts: CatalogFacts | undefined) => (facts?.lifts ?? []).find(([, metres]) => metres <= 300)?.[0];
const size = (park: FactsPlace | undefined) => park?.size === 'large' ? 'large' : 'small';

/** The steps of the cheapest way to a row, as the row shows them. */
export function accessSteps(row: Row, place: Place): Step[] {
    const way = row.ways[0], walk: Step[] = 'walk' in way && way.walk >= SHOWN_WALK_M ? [{ icon: 'walk', text: walked(way.walk) }] : [];
    const drive = (metres: number): Step => ({ icon: 'car', text: formatDistance(metres) });
    switch (way.kind) {
        case 'any': {
            if (row.shape === 'liftDown' || row.shape === 'liftUp') return [{ icon: 'lift', text: 'Lift' }];
            const park = place(parkAt(row.facts, place) ?? '');
            if (park) return [{ park: true, text: `${size(park) === 'large' ? 'Large' : 'Small'} car park` }];
            if (liftAt(row.facts)) return [{ icon: 'lift', text: 'Lift' }];
            return row.facts ? [{ text: 'No car park' }] : [];
        }
        case 'foot': return [{ icon: 'walk', text: walked(way.walk) }];
        case 'car': return [drive(way.drive), { park: true, text: size(place(way.park)) }, ...walk];
        case 'pin': return [place(way.place)?.kind === 'lift' ? { icon: 'lift', text: 'Lift' } : { park: true, text: size(place(way.place)) }, ...walk.length ? walk : [{ text: 'starts there' }]];
        default: {
            const before: Step[] = way.drive !== undefined ? [drive(way.drive)] : way.before >= SHOWN_WALK_M ? [{ icon: 'walk', text: walked(way.before) }] : [];
            return [...before, { icon: 'lift', text: 'Lift' }, ...walk];
        }
    }
}

/** The way to a row in one sentence, for the card. */
export function accessText(row: Row, place: Place): string {
    const way: Way = row.ways[0];
    const then = 'walk' in way ? way.walk >= SHOWN_WALK_M ? `, then walk ${walked(way.walk)} to the route` : ', the route starts there' : '';
    const liftOf = (id: string) => place(id)?.name ?? 'the lift';
    switch (way.kind) {
        case 'any': {
            if (row.shape === 'liftDown' || row.shape === 'liftUp') return '';
            const park = place(parkAt(row.facts, place) ?? ''), lift = place(liftAt(row.facts) ?? '');
            if (park) return `${park.name ?? 'A car park'} (${size(park)}) is at the route.`;
            if (lift) return `${lift.name ?? 'A lift'} stops on the route.`;
            return row.facts ? 'No car park at the route.' : '';
        }
        case 'foot': return `Walk ${walked(way.walk)} from your start to the route.`;
        case 'car': return `Drive ${formatDistance(way.drive)} to ${place(way.park)?.name ?? 'a car park'} (${size(place(way.park))})${then}.`;
        case 'pin': return `Start at ${placeName(place(way.place))}${then}.`;
        case 'lift': return `${way.drive !== undefined ? `Drive ${formatDistance(way.drive)}` : `Walk ${walked(way.before)}`} to ${liftOf(way.lift)}, ride up${then}.`;
        default: return `${way.drive !== undefined ? `Drive ${formatDistance(way.drive)}` : `Walk ${walked(way.before)}`} to the valley station of ${liftOf(way.lift)}.`;
    }
}

/** What the shape means for the rider, with the numbers that a row does not show. */
export function shapeText(row: Row): string {
    if (row.shape === 'back') return `There and back, ${formatDistance(row.length_m / 2)} each way; length and climb count both ways`;
    if (row.shape === 'liftDown') return `Hike up ${formatHeight(row.ascent_m)} and down ${formatHeight(row.descent_m)}, then the lift down; park at the valley station`;
    if (row.shape === 'liftUp') return `The lift up, then hike down ${formatHeight(row.descent_m)} and up ${formatHeight(row.ascent_m)}; park at the valley station`;
    return 'Loop';
}

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
/** The first day of a day step, or with `last` its last day, from 1 September of a year with no 29 February. */
function stepDay(step: number, last = false): string {
    const day = new Date(Date.UTC(2025, 8, 1 + 2 * step + (last ? 1 : 0)));
    return `${day.getUTCDate()} ${MONTHS[day.getUTCMonth()]}`;
}

/** "Usually free of snow 14 Jun to 17 Oct", or nothing with no data. */
export function snowText(free: CatalogFacts['snow_free']): string | undefined {
    if (!free) return undefined;
    if (free.length === 0) return 'No day is usually free of snow';
    if (free[0] === 0 && free[1] === 182) return 'Usually free of snow all year';
    return `Usually free of snow ${stepDay(free[0])} to ${stepDay(free[1], true)}`;
}

/** The words of a change that helps an empty list. */
export function suggestionText(suggestion: Suggestion, query: Query, place: Place): string {
    const next = suggestion.query;
    switch (suggestion.kind) {
        case 'nice': return `${wishLabel(suggestion.wish, place, true)} as a wish only`;
        case 'helps': return 'A lift only when it helps';
        case 'part': return `No T${query.grade[0] + 1} part needed`;
        case 'walk': return `Walk up to ${walked(next.walk)}`;
        case 'drive': return `Drive up to ${next.drive} km`;
        case 'car': return 'Take the car';
        case 'lifts': return 'Take lifts';
        case 'unpin': return 'Unpin the start';
        case 'shape': return `${SHAPE_WORDS[suggestion.shape].label} too`;
        case 'length': return `Length ${rangeText(next.length!, shortDistance).toLowerCase()}`;
    }
}

export const hikes = (count: number) => `${count.toLocaleString('en')} ${count === 1 ? 'hike' : 'hikes'}`;
