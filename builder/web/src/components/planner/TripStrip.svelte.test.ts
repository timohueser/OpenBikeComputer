// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { planView, type Trip } from '../../lib/planner/editor';
import { weekOf } from '../../lib/planner/week';
import { testLine, testTrip } from '../../../test-support/planner/trip';
import TripStrip from './TripStrip.svelte';

let strip: ReturnType<typeof mount> | undefined;
afterEach(async () => { if (strip) await unmount(strip); document.body.replaceChildren(); });

/** A strip of a 32-day trip, 32 slots of 30 px with 2 px gaps: day d spans 32 (d − 1) to 32 (d − 1) + 30 px. */
function mountStrip() {
    const trip: Trip = { ...testTrip(), days: 32 };
    const days = planView(trip, testLine(trip)).days;
    let start = $state(1);
    const moves: number[] = [];
    strip = mount(TripStrip, { target: document.body, props: { days, get week() { return weekOf(days.length, start)!; }, theme: 'light',
        onMove: (next: number) => { moves.push(next); start = next; } } });
    flushSync();
    const slider = document.querySelector<HTMLElement>('[role="slider"]')!;
    vi.spyOn(slider, 'getBoundingClientRect').mockReturnValue(new DOMRect(0, 0, 32 * 32 - 2, 40));
    slider.setPointerCapture = vi.fn();
    return { slider, moves, start: () => start };
}

describe('trip strip', () => {
    it('moves the week with the keys of a slider', () => {
        const { slider, start } = mountStrip();
        const key = (key: string) => { slider.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true })); flushSync(); return start(); };
        expect(slider.getAttribute('aria-valuetext')).toBe('Days 1 to 7');
        expect(['ArrowRight', 'PageUp', 'ArrowLeft', 'End', 'PageDown', 'ArrowDown', 'Home', 'ArrowLeft'].map(key)).toEqual([2, 9, 8, 26, 19, 18, 1, 1]);
        expect(slider.getAttribute('aria-valuetext')).toBe('Days 1 to 7');
    });

    it('takes the focus at a press, keeps the day under the pointer while it drags, and centres the week on a day outside it', () => {
        const { slider, moves, start } = mountStrip();
        const pointer = (type: string, day: number) => {
            slider.dispatchEvent(new PointerEvent(type, { clientX: 32 * (day - 1) + 15, button: 0, pointerId: 1, bubbles: true }));
            flushSync();
            return start();
        };
        pointer('pointerdown', 3);
        expect(document.activeElement).toBe(slider);
        expect(moves).toEqual([]);
        expect(pointer('pointermove', 7)).toBe(5);
        pointer('pointerup', 7);
        expect(pointer('pointerdown', 20)).toBe(17);
        expect(pointer('pointermove', 22)).toBe(19);
        pointer('pointerup', 22);
        expect(moves).toEqual([5, 17, 19]);
    });
});
