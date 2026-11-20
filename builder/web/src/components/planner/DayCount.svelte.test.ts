// @vitest-environment happy-dom
import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, describe, expect, it } from 'vitest';
import { maxRidingDays, setDays, type Trip } from '../../lib/planner/editor';
import { testLine, testTrip } from '../../../test-support/planner/trip';
import DayCount from './DayCount.svelte';

let row: ReturnType<typeof mount> | undefined;
afterEach(async () => { if (row) await unmount(row); document.body.replaceChildren(); });

const button = (label: string) => document.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)!;
const count = () => document.querySelector('.count span')!;

describe('day count', () => {
    it('makes a route a trip with a dashed guess, steps it, and makes it a route again at its lowest count', async () => {
        const base = testTrip();
        let trip = $state.raw<Trip>({ ...base, mode: 'route', daysSet: undefined });
        let said = '';
        row = mount(DayCount, { target: document.body, props: { get trip() { return trip; }, line: testLine(base),
            onChange: (next: Trip, description: string) => { trip = next; said = description; }, onLimit: () => {} } });
        flushSync();
        expect(count().textContent).toBe('1 day');
        expect(button('One day fewer').disabled).toBe(true);

        button('One day more').click(); flushSync();
        expect(trip).toMatchObject({ mode: 'trip', days: 2 });
        expect(said).toBe('Multi-day trip, 2 days');
        expect(count().textContent).toBe('2 days');
        expect(count().classList.contains('guess')).toBe(true);

        button('One day more').click(); flushSync();
        expect(trip).toMatchObject({ days: 3, daysSet: true });
        expect(count().classList.contains('guess')).toBe(false);
        button('One day fewer').click(); flushSync();
        button('One day fewer').focus();
        button('One day fewer').click(); flushSync();
        await tick();
        expect(document.activeElement).toBe(button('One day more'));
        expect(trip).toMatchObject({ mode: 'route', days: 2 });
        expect(said).toBe('One-day route · + brings back the days');
        expect(count().textContent).toBe('1 day');
        expect(button('One day fewer').disabled).toBe(true);
    });

    it('keeps the + in the focus order at the most days, and says why it adds no day', () => {
        const trip = setDays(testTrip(), maxRidingDays);
        let limits = 0, changes = 0;
        row = mount(DayCount, { target: document.body, props: { trip, line: undefined, onChange: () => changes++, onLimit: () => limits++ } });
        flushSync();
        expect(count().textContent).toBe(`${maxRidingDays} days`);
        button('One day more').focus();
        button('One day more').click(); flushSync();
        expect([limits, changes, button('One day more').getAttribute('aria-disabled')]).toEqual([1, 0, 'true']);
        expect(document.activeElement).toBe(button('One day more'));
    });
});
