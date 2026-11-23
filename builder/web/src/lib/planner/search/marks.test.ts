import { describe, expect, it } from 'vitest';
import { scan } from '../../ui/search-bar/read';
import { areasAndTimes, PLANNER_WORDS, saysNearest, type Mark } from './marks';
import { NEAREST } from './slots';

/** Each marked phrase of a text with the label and the slot of its value. */
const marked = (text: string) => scan(text, PLANNER_WORDS).flatMap(part => part.kind === 'value' ? [[text.slice(part.from, part.to), (part.value as Mark).label, part.value.slot]] : []);
/** The slots after the marked values of a text, in their order. */
const applied = (text: string) => scan(text, PLANNER_WORDS).reduce((slots, part) => part.kind === 'value' ? part.value.set(slots) : slots, {});

describe('the words of the planner search', () => {
    it('marks the kinds, the facts, the days with their parts and the opening times of the examples', () => {
        expect(marked('drinking water in the middle of day 1')).toEqual([['drinking water', 'Drinking water', 'find'], ['middle', 'Middle', 'where'], ['day 1', 'Day 1', 'where']]);
        expect(marked('climbs over 300 m on day 2')).toEqual([['climbs', 'Climbs', 'find'], ['day 2', 'Day 2', 'where']]);
        expect(marked('supermarket open now')).toEqual([['supermarket', 'Supermarket', 'find'], ['open now', 'Open now', 'when']]);
        expect(marked('a cafe open on Sundays near me')).toEqual([['cafe', 'Cafe', 'find'], ['open on Sundays', 'Open on Sunday', 'when'], ['near me', 'Near me', 'where']]);
    });

    it('marks an action as the label of "Find", and "the next" of a place as the nearest one ahead, not a stretch', () => {
        expect(marked('route to the next supermarket')).toEqual([['route to', 'Route to', 'find'], ['next', 'Nearest to me', 'where'], ['supermarket', 'Supermarket', 'find']]);
        expect(applied('route to the next supermarket')).toEqual({ act: 'route', find: { kind: 'supermarket' }, where: NEAREST });
        expect(marked('add a stop at a bakery')).toEqual([['add a stop at', 'Add a stop at', 'find'], ['bakery', 'Bakery', 'find']]);
        expect(marked('sleep at a campsite')).toEqual([['sleep at', 'End the day at', 'find'], ['campsite', 'Campsite', 'find']]);
        expect(saysNearest('the closest cafe')).toBe(true);
        // Only before a kind of place: "the next 20 km" is a stretch, which the query model reads.
        for (const text of ['campsite next to a lake', 'bakery open next sunday', 'water in the next twenty km', 'water in the next 20 km', 'next week'])
            expect(saysNearest(text), text).toBe(false);
        // "Next Sunday" alone goes to the query model.
        expect(areasAndTimes('next sunday')).toBeUndefined();
    });

    it('puts a part and a day of a sentence together in one search area', () => {
        expect(applied('in the middle of day 1')).toEqual({ where: { day: 1, part: 'middle' } });
        expect(applied('second day, at the end')).toEqual({ where: { day: 2, part: 'end' } });
        expect(applied('the end of the route')).toEqual({ where: { scope: 'route', part: 'end' } });
        expect(applied('tonight')).toEqual({ where: { day: 'today', part: 'end' } });
    });

    it('reads with no model only a sentence of areas and times whose other words carry no meaning', () => {
        expect(areasAndTimes('on day 2, open now')?.map(mark => mark.label)).toEqual(['Day 2', 'Open now']);
        expect(areasAndTimes('end of day 2')?.map(mark => mark.label)).toEqual(['End', 'Day 2']);
        expect(areasAndTimes('in the middle of day 1')?.map(mark => mark.label)).toEqual(['Middle', 'Day 1']);
        for (const text of ['open tomorrow', 'open on day 2', 'after day 2', 'between day 1 and day 2', 'water on day 2', 'day 1 or day 2',
            'tonight and tomorrow', 'over day 2', 'around day 2', 'day 2 near me']) expect(areasAndTimes(text), text).toBeUndefined();
        // An exact term wins over a plural of another: each morning, not the start of a day.
        expect(marked('mornings')).toEqual([['mornings', 'Start of every day', 'where']]);
    });
});
