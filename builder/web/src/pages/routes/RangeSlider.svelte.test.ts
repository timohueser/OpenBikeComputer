// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it } from 'vitest';
import type { Range } from './filter';
import RangeSlider from './RangeSlider.svelte';

let slider: ReturnType<typeof mount> | undefined;
afterEach(async () => { if (slider) await unmount(slider); document.body.replaceChildren(); });

// 20 values: the value at 90 % is 79, so the scale has the step 5 and the top 80.
const VALUES = [...Array(11).fill(1), 0, 3, 7, 12, 40, 41, 79, 79, 300];

function show(values: number[], start?: Range, fixed?: number) {
    let range = $state(start);
    const changes: (Range | undefined)[] = [];
    slider = mount(RangeSlider, { target: document.body, props: {
        values, get range() { return range; }, fixed, label: 'Length', format: (value: number) => `${value} km`,
        onchange: (next: Range | undefined) => { changes.push(next); range = next; },
    } });
    flushSync();
    return { changes, range: () => range };
}

const [from, to] = [() => input('Length, from'), () => input('Length, to')];
const input = (label: string) => document.querySelector<HTMLInputElement>(`input[aria-label="${label}"]`)!;
const reading = () => document.querySelector('.reading')!.textContent;
const bars = () => [...document.querySelectorAll<HTMLElement>('.bars span')];

function set(element: HTMLInputElement, value: number) {
    element.value = String(value);
    element.dispatchEvent(new Event('input', { bubbles: true }));
    flushSync();
}

describe('range slider', () => {
    it('gives no inverted range when a stored end lies beyond the scale', () => {
        // A fixed step of 5 and both ends beyond the top of 80: the handles stand at 70 and 75.
        const { changes } = show(VALUES, [100, 120], 5);
        set(to(), 40);
        expect(changes.at(-1)).toEqual([70, 75]);
    });

    it('counts the values of each step, and the last bar counts each value from the top up', () => {
        show(VALUES);
        // The highest bar holds 13 values; the last one holds 79, 79 and 300, and is striped for the 300 above the top.
        expect(bars().map(bar => Math.round(parseFloat(bar.style.getPropertyValue('--share')) * 13))).toEqual([13, 1, 1, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 3]);
        expect(bars().map(bar => bar.classList.contains('more'))).toEqual([...Array(15).fill(false), true]);
        // An empty step is a stub; a bar with a count is not.
        expect(bars().map(bar => bar.classList.contains('empty'))).toEqual([false, false, false, ...Array(5).fill(true), false, ...Array(6).fill(true), false]);
        expect(to().max).toBe('80');
        expect(reading()).toBe('Any');
        expect([...document.querySelectorAll('.ends span')].map(end => end.textContent)).toEqual(['0', '75 km+']);
    });

    it('sets the range with a handle, and gives no range with both handles at the ends', () => {
        const { changes } = show(VALUES);
        set(to(), 20);
        expect(changes.at(-1)).toEqual([0, 20]);
        expect(reading()).toBe('Under 20 km');
        expect(bars().map(bar => bar.classList.contains('on'))).toEqual([true, true, true, true, ...Array(12).fill(false)]);
        set(from(), 10);
        expect(changes.at(-1)).toEqual([10, 20]);
        set(to(), 80);
        expect(changes.at(-1)).toEqual([10, Infinity]);
        set(from(), 0);
        expect(changes.at(-1)).toBeUndefined();
        expect(reading()).toBe('Any');
    });

    it('stops a handle one step before the other one', () => {
        const { changes } = show(VALUES, [10, 30]);
        set(from(), 50);
        expect(changes.at(-1)).toEqual([25, 30]);
        expect(from().value).toBe('25');
        set(to(), 0);
        expect(changes.at(-1)).toEqual([25, 30]);
        expect(to().value).toBe('30');
    });

    it('keeps its scale while it shows, so a handle does not move under the pointer', () => {
        // The end 300 gives the step 20; the range [0, 100) alone would give the step 10.
        const { range } = show(VALUES, [0, 300]);
        set(to(), 100);
        expect(range()).toEqual([0, 100]);
        expect([to().step, to().max, to().value]).toEqual(['20', '320', '100']);
    });

    it('shows a set end that is near the end of the rail one step in, so a move to the end clears it', () => {
        const { changes } = show(VALUES, [0.4, Infinity]);
        expect(from().value).toBe('5');
        expect(reading()).toBe('0.4 km or more');
        set(from(), 0);
        expect(changes).toEqual([undefined]);
    });

    it('shows a set upper end near the end of the rail one step in, so a move to the end clears it', () => {
        const { changes } = show(VALUES, [0, 79]);
        expect(to().value).toBe('75');
        set(to(), 80);
        expect(changes).toEqual([undefined]);
    });

    it('keeps a stored end that is off the step until the rider moves its handle', () => {
        const { changes, range } = show(VALUES, [12, 33]);
        expect([from().value, to().value]).toEqual(['10', '35']);
        set(to(), 50);
        expect(range()).toEqual([12, 50]);
        set(from(), 20);
        expect(changes).toEqual([[12, 50], [20, 50]]);
    });
});
