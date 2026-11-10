// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it } from 'vitest';
import type { Trip } from '../../lib/planner/editor';
import { testTrip } from '../../../test-support/planner/trip';
import TripRow from './TripRow.svelte';

let row: ReturnType<typeof mount> | undefined;
afterEach(async () => { if (row) await unmount(row); document.body.replaceChildren(); });

const button = (label: string) => document.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)!;

describe('trip row', () => {
    it('draws a guessed day count dashed until the rider steps it, and keeps the kept nights', () => {
        const base = testTrip();
        let trip = $state<Trip>({ ...base, daysSet: undefined, points: [...base.points, { id: 'night-2', kind: 'night', night: 2, label: 'Olten', coordinate: [7.6, 47.3] }] });
        row = mount(TripRow, { target: document.body, props: { get trip() { return trip; }, onChange: (next: Trip) => trip = next } });
        flushSync();
        const count = () => document.querySelector('.count span')!;
        expect(count().textContent).toBe('3 riding days');
        expect(count().classList.contains('guess')).toBe(true);
        expect(button('One day fewer').disabled).toBe(true);

        button('One day more').click(); flushSync();
        expect(count().textContent).toBe('4 riding days');
        expect(count().classList.contains('guess')).toBe(false);
        button('One day fewer').click(); flushSync();
        expect(trip).toMatchObject({ days: 3, daysSet: true });
        expect(button('One day fewer').disabled).toBe(true);
    });
});
