// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it } from 'vitest';
import type { RouteNote } from '../../lib/planner/route-notes';
import RouteNotes from './RouteNotes.svelte';

let notes: ReturnType<typeof mount> | undefined;
afterEach(async () => { if (notes) await unmount(notes); document.body.replaceChildren(); });

describe('route notes', () => {
    it('opens a closed count row into one entry per stretch, and an entry shows its stretch and its state', () => {
        const list: RouteNote[] = [
            { icon: 'lock', label: 'Customers only', km: 41.23, length: 0.339, coordinates: [[[7, 47], [7.01, 47]]] },
            { icon: 'flag', label: 'Ends 590 m before your point', km: 113.9, coordinates: [[[8, 47], [8, 47.005]]] },
            { icon: 'lock', label: 'May be closed Oct–May', km: 104.62, end: 118.31, length: 9.4, coordinates: [[[9, 46], [9.01, 46]], [[9.03, 46], [9.04, 46]]] },
        ];
        let shown = $state.raw<RouteNote>();
        notes = mount(RouteNotes, { target: document.body, props: { notes: list, get shown() { return shown; }, onShow: (note: RouteNote) => shown = note } });
        flushSync();
        const toggle = document.querySelector<HTMLButtonElement>('[aria-expanded]')!;
        expect(toggle.textContent).toBe('3 things to check');
        expect(toggle.getAttribute('aria-expanded')).toBe('false');
        expect(document.querySelectorAll('li')).toHaveLength(0);

        toggle.click(); flushSync();
        expect(toggle.getAttribute('aria-expanded')).toBe('true');
        const entries = [...document.querySelectorAll<HTMLButtonElement>('li button')];
        expect(entries.map(entry => entry.textContent)).toEqual(['Customers onlykm 41.2 · 339 m', 'Ends 590 m before your pointkm 113.9',
            'May be closed Oct–Maykm 104.6 – 118.3']);
        entries[0].click(); flushSync();
        expect(shown).toBe(list[0]);
        expect(entries.map(entry => entry.getAttribute('aria-pressed'))).toEqual(['true', 'false', 'false']);
    });

    it('shows the length of a merged note whose start and end print the same', () => {
        const note: RouteNote = { icon: 'lock', label: 'Private road', km: 10.01, end: 10.04, length: 0.02, coordinates: [] };
        notes = mount(RouteNotes, { target: document.body, props: { notes: [note], onShow: () => {} } });
        flushSync();
        document.querySelector<HTMLButtonElement>('[aria-expanded]')!.click(); flushSync();
        expect(document.querySelector('li small')!.textContent).toBe('km 10.0 · 20 m');
    });

    it('says "1 thing" for one note and draws nothing for none', async () => {
        const note: RouteNote = { icon: 'mountain', label: 'Alpine path (T4)', km: 3, length: 2.1, coordinates: [] };
        notes = mount(RouteNotes, { target: document.body, props: { notes: [note], onShow: () => {} } });
        flushSync();
        expect(document.querySelector('[aria-expanded]')!.textContent).toBe('1 thing to check');
        await unmount(notes);
        notes = mount(RouteNotes, { target: document.body, props: { notes: [], onShow: () => {} } });
        flushSync();
        expect(document.body.querySelector('button')).toBeNull();
    });
});
