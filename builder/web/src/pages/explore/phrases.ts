/** The words that the text field of Explore understands, in English, and what each does to the query. The same table
 * marks the phrases while the rider types and reads them at Enter. A number is in the units of the rider. */
import type { FactsPlace } from '../../lib/planner/signed-routes';
import { distanceIn, formatHeight, heightIn, heightUnit } from '../../lib/settings/format';
import { fold, type Table, type Value } from '../../lib/ui/search-bar/read';
import { rangeText, type Range } from '../routes/filter';
import { defaultQuery, DRIVE_KM, find, GRADE_TOP, SHAPES, WISHES, type Loaded, type Query, type Shape, type Wish } from './search';
import { gradeText, shapesSet, type SlotId } from './slots';
import { LENGTH_TOP_M, SHAPE_WORDS, shortDistance, wishIcon, wishLabel } from './words';

/** A value of the field: a change of the query. `here` also asks the browser for the start point when the query has no
 * start point and no pin. */
export interface Choice extends Value { slot: SlotId; set: (query: Query) => Partial<Query>; here?: true }

type Place = (id: string) => FactsPlace | undefined;
type Named = typeof WISHES[number];

/** The query after the choices, in their order. */
export const apply = (query: Query, choices: Choice[]): Query => choices.reduce((next, choice) => ({ ...next, ...choice.set(next) }), query);

/** Whether a choice changes the query, or asks for the start point that the query does not have. */
export const changes = (query: Query, choice: Choice) =>
    (!!choice.here && !query.start && !query.pin) || JSON.stringify(apply(query, [choice])) !== JSON.stringify(query);

// ── Values ──────────────────────────────────────────────────────────────────

const wish = (id: Wish, place: Place): Choice =>
    ({ label: wishLabel(id, place, true), slot: 'way', icon: wishIcon(id, place), set: query => ({ wishes: { ...query.wishes, [id]: 'must' } }) });

const SHAPE_ICONS: Record<Shape, string> = { loop: 'loop', back: 'undo', liftDown: 'lift', liftUp: 'lift' };
/** A shape joins the accepted shapes; with all of them accepted, it is the only one. A lift shape turns lifts on. */
const shape = (id: Shape): Choice => ({
    label: SHAPE_WORDS[id].tag, slot: 'hike', icon: SHAPE_ICONS[id],
    set: query => {
        const accepted = shapesSet(query), lift = id === 'liftDown' || id === 'liftUp';
        return { shapes: accepted.length ? SHAPES.filter(one => one === id || accepted.includes(one)) : [id], ...lift && !query.lift ? { lift: 'helps' as const } : {} };
    },
});

const length = (range: Range): Choice => ({ label: rangeText(range, shortDistance), slot: 'hike', icon: 'route', set: () => ({ length: range }) });
const climb = (range: Range): Choice => ({ label: `${rangeText(range, formatHeight)} climb`, slot: 'hike', icon: 'ascent', set: () => ({ climb: range }) });
const grade = (range: [number, number]): Choice => ({ label: gradeText({ grade: range })!, slot: 'hike', icon: 'hiking', set: () => ({ grade: range }) });
/** A drive in km, in the steps of its slider. A drive needs a start point, as the car does. */
function drive(km: number): Choice {
    const value = Math.min(DRIVE_KM.max, Math.max(DRIVE_KM.min, Math.round(km / DRIVE_KM.step) * DRIVE_KM.step));
    return { label: `By car, up to ${value} km`, slot: 'from', icon: 'car', set: () => ({ car: true, drive: value }), here: true };
}
const HERE: Choice = { label: 'My location', slot: 'from', icon: 'locate', set: () => ({}), here: true };
const CAR: Choice = { label: 'By car', slot: 'from', icon: 'car', set: () => ({ car: true }), here: true };
const LIFTS: Choice = { label: 'Lifts', slot: 'from', icon: 'lift', set: query => ({ lift: query.lift ?? 'helps' }) };

/** A length or a climb in the units of the rider. */
const lengthIn = (count: number) => count / distanceIn(1);
const usualClimb = () => heightUnit() === 'm' ? 600 : 2000 / heightIn(1);

// ── Words ───────────────────────────────────────────────────────────────────

const WISH_WORDS: Record<Named, string[]> = {
    peak: ['summit', 'peak', 'top', 'mountain'],
    lake: ['lake', 'tarn', 'pond', 'swim', 'swimming'],
    hut: ['hut with food', 'hut', 'mountain hut', 'food', 'restaurant', 'cafe', 'inn', 'lunch', 'refuge'],
    waterfall: ['waterfall', 'falls', 'cascade'],
    official: ['official route', 'official', 'signposted', 'waymarked', 'marked'],
    snow: ['free of snow', 'usually free of snow', 'snow free', 'snowfree'],
    quiet: ['far from big roads', 'quiet', 'remote', 'peaceful', 'away from roads'],
    forest: ['mostly forest', 'forest', 'woods', 'woodland', 'trees', 'shady', 'shade'],
    open: ['mostly open', 'open', 'open terrain', 'meadows', 'views', 'above the trees'],
    water: ['along water', 'river', 'stream', 'riverside', 'creek'],
};
/** "no" before these wishes gives the other one. The pairs are approximate: a hike with 21 % to 59 % forest is neither. */
const OPPOSITE: Partial<Record<Named, Named>> = { forest: 'open', open: 'forest' };
/** The search can keep only hikes that have a wish: "no" before these says so. */
const LEFT_OUT: Partial<Record<Named, string>> = {
    peak: 'hikes with a summit', lake: 'hikes with a lake', hut: 'hikes with a hut', waterfall: 'hikes with a waterfall', official: 'official routes', water: 'hikes along water',
};
/** Words that mean nothing alone and a wish after "no". */
const ONLY_NOT: [string[], Named][] = [[['snow'], 'snow'], [['big roads', 'busy roads', 'traffic', 'cars'], 'quiet']];

const SHAPE_PHRASES: Record<Shape, string[]> = {
    loop: ['loop', 'round trip', 'roundtrip', 'circular', 'circuit'],
    back: ['there and back', 'out and back', 'same way back', 'return trip'],
    liftUp: ['lift up', 'cable car up', 'gondola up', 'ride up'],
    liftDown: ['lift down', 'cable car down', 'gondola down', 'ride down'],
};
const BIKES = ['bike', 'biking', 'bicycle', 'cycling', 'cycle', 'mtb', 'mountain bike', 'gravel', 'road bike'];

const LESS = ['under', 'below', 'up to', 'at most', 'max', 'less than', 'shorter than'];
const MORE = ['at least', 'over', 'above', 'more than', 'longer than', 'min'];
const ABOUT = ['about', 'around', 'roughly'];
const WITHIN = ['within', 'drive', 'driving'];
/** The metres of each unit that can follow a number. */
const LENGTHS: Record<string, number> = { km: 1000, k: 1000, kilometres: 1000, kilometers: 1000, mi: 1609.344, mile: 1609.344, miles: 1609.344 };
const HEIGHTS: Record<string, number> = { m: 1, metres: 1, meters: 1, hm: 1, ft: 0.3048, feet: 0.3048 };
/** A number with no unit is a length up to twice the scale of the slider, and a climb from 50 m. */
const LONGEST_M = 2 * LENGTH_TOP_M, LEAST_CLIMB_M = 50;

const GLUE = ('a an the with and or in on of for to at by from near around along via past passing passes through i we me my want would like please '
    + 'show find some also but plus that has have is it be go goes which where there one nice hike hikes hiking walk walks walking route routes trail trails tour').split(' ');

type Kind = 'less' | 'more' | 'about';
const kindOf = (lead: string | undefined): Kind | undefined => !lead ? undefined : LESS.includes(lead) ? 'less' : MORE.includes(lead) ? 'more' : 'about';
/** "1,500" is a thousand and five hundred, and "1,5" one and a half. */
const number = (text: string) => Number(/^\d{1,3}(,\d{3})+$/.test(text) ? text.replaceAll(',', '') : text.replace(',', '.'));

/** The ranges of a number in a unit of `size` metres: up to it, about it (a fifth less to a fifth more) and from it. A
 * word before it keeps one of them. */
function ranges(count: number, size: number, kind: Kind | undefined): Range[] {
    const about: Range = [Math.round(count * 0.8) * size, Math.round(count * 1.2) * size];
    const all: Record<Kind, Range | undefined> = { less: [0, count * size], about: about[0] < about[1] ? about : undefined, more: [count * size, Infinity] };
    return (kind ? [all[kind]] : [all.less, all.about, all.more]).filter((range): range is Range => !!range);
}

/** The lengths and the climbs that a number can mean, the lengths first. `count` decides for a number with no unit. */
function sized(count: number, unit: string | undefined, climbed: boolean, make: (size: number) => Range[]): Choice[] {
    if (!(count > 0)) return [];
    const along = unit ? LENGTHS[unit] : climbed ? undefined : 1 / distanceIn(1), up = unit ? HEIGHTS[unit] : 1 / heightIn(1);
    return [...along && (unit || count * along <= LONGEST_M) ? make(along).map(length) : [],
        ...up && (unit || climbed || count * up >= LEAST_CLIMB_M) ? make(up).map(climb) : []];
}

function grades(index: number, kind: Kind | undefined): Choice[] {
    const range: [number, number] = kind === 'more' ? [index, GRADE_TOP] : [0, index];
    return range[0] > 0 || range[1] < GRADE_TOP ? [grade(range)] : [];
}

const words = (list: string[]) => [...list].sort((a, b) => b.length - a.length).join('|');
const N = '(\\d{1,3}(?:,\\d{3})+(?!\\d)|\\d+(?:[.,]\\d+)?)', END = '(?![\\p{L}\\d])', UNIT = `(${words([...Object.keys(LENGTHS), ...Object.keys(HEIGHTS)])})`;
const CLIMBED = '(\\s+(?:of\\s+)?(?:climb|climbing|ascent|elevation gain|uphill))?';
const LEAD = `(?:(${words([...LESS, ...MORE, ...ABOUT])})\\s+)?`;

/** Why the page does not search a choice, such as "Not for long trails"; nothing for a choice that it searches. */
export type Off = (choice: Choice) => string | undefined;

/** The choices that change nothing that the page searches, where `searched` gives the query that it searches. */
export function ignored(searched: (query: Query) => Query, why: string, today: string): Off {
    const base = defaultQuery(today), seen = JSON.stringify(searched(base));
    return choice => JSON.stringify(searched(apply(base, [choice]))) === seen ? why : undefined;
}

/** The table of the field, with "Passes <name>" for each named summit, hut, lake and waterfall that the loaded hikes pass.
 * A choice that is off is a note, so its words stay in the field. */
export function phraseTable(loaded: Loaded, off: Off): Table<Choice> {
    const place = loaded.place, phrases: Table<Choice>['phrases'] = [];
    const note = (choice: Choice) => `${off(choice)}: ${choice.label}.`;
    const usable = (value?: Choice | string) => typeof value === 'object' && off(value) ? note(value) : value;
    const on = (choices: Choice[]) => choices.filter(choice => !off(choice));
    /** The values of a quantity; with each of them off, a note, so no shorter quantity reads the same number. */
    const read = (choices: Choice[]) => on(choices).length || !choices.length ? on(choices) : note(choices[0]);
    const add = (list: string[], value?: Choice | string, not?: Choice | string) => phrases.push(...list.map(text => ({ words: text, value: usable(value), not: usable(not) })));
    for (const id of WISHES) {
        const opposite = OPPOSITE[id], left = LEFT_OUT[id];
        add([fold(wishLabel(id, place)), ...WISH_WORDS[id]], wish(id, place), opposite ? wish(opposite, place) : left && `Explore cannot leave out ${left}.`);
    }
    for (const [list, id] of ONLY_NOT) add(list, undefined, wish(id, place));
    for (const id of SHAPES) add(SHAPE_PHRASES[id], shape(id));
    add(['easy'], grade([0, 1]));
    add(['alpine'], grade([3, GRADE_TOP]));
    add(['near me', 'from here', 'my location', 'nearby', 'close to me', 'around me'], HERE);
    add(['by car', 'car'], CAR);
    add(['lift', 'with lifts', 'cable car', 'gondola', 'chairlift'], LIFTS);
    add(BIKES, 'Explore searches hikes only for now.');
    const named = new Map<string, string>();
    for (const record of loaded.records) {
        for (const [id] of loaded.factsOf(record.id)?.places ?? []) {
            const found = place(id), name = found?.name && fold(found.name).replace(/[^\p{L}\p{N}]+/gu, ' ').trim();
            if (name && found.kind !== 'parking' && found.kind !== 'lift' && !named.has(name)) named.set(name, id);
        }
    }
    for (const [name, id] of named) add([name], wish(`via:${id}`, place));
    return {
        phrases,
        patterns: [
            { match: new RegExp(`^${LEAD}${N}\\s*${UNIT}?${END}${CLIMBED}`, 'u'),
                values: ([, lead, count, unit, climbed]) => read(sized(number(count), unit, !!climbed, size => ranges(number(count), size, kindOf(lead)))) },
            { match: new RegExp(`^(?:between\\s+)?${N}\\s*(?:-|–|to|and)\\s*${N}\\s*${UNIT}?${END}${CLIMBED}`, 'u'),
                values: ([, low, high, unit, climbed]) => number(low) < number(high) ? read(sized(number(high), unit, !!climbed, size => [[number(low) * size, number(high) * size]]).slice(0, 1)) : [] },
            { match: new RegExp(`^${LEAD}t([1-6])${END}`, 'u'), values: ([, lead, index]) => read(grades(Number(index) - 1, kindOf(lead))) },
            { match: new RegExp(`^(?:${words(WITHIN)})\\s+(?:up to\\s+)?${N}\\s*(?:km)?${END}`, 'u'), values: ([, count]) => read([drive(number(count))]) },
        ],
        starts: [
            { words: LESS, values: on([length([0, lengthIn(10)]), climb([0, usualClimb()]), grade([0, 1])]) },
            { words: MORE, values: on([length([lengthIn(10), Infinity]), climb([usualClimb(), Infinity]), grade([2, GRADE_TOP])]) },
            { words: ABOUT, values: on([length([lengthIn(8), lengthIn(12)]), length([lengthIn(12), lengthIn(18)])]) },
            { words: WITHIN, values: on([drive(20), drive(30)]) },
        ],
        units: [...Object.keys(LENGTHS), ...Object.keys(HEIGHTS)],
        glue: GLUE,
        not: ['no', 'not', 'without', 'avoid'],
    };
}

/** Up to four values that riders often add next: those that the page searches, that the query does not hold and that leave
 * a hike. */
export function often(query: Query, loaded: Loaded, off: Off): Choice[] {
    return [length([0, lengthIn(12)]), shape('loop'), wish('hut', loaded.place), wish('peak', loaded.place), climb([0, usualClimb()]), wish('lake', loaded.place)]
        .filter(choice => !off(choice) && changes(query, choice) && find(apply(query, [choice]), loaded).length > 0).slice(0, 4);
}

/** Sentences for an empty field before the first search. */
export const EXAMPLES = ['loop with a hut, under 15 km', 'summit hike up to T3, no forest', 'waterfall and lake, under 800 m climb'];
