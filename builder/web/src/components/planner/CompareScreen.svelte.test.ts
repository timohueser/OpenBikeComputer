// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Trip } from '../../lib/planner/editor';
import { routingKey } from '../../lib/planner/routing';
import type { Version } from '../../lib/planner/versions';
import { testLine, testTrip } from '../../../test-support/planner/trip';
import CompareScreen from './CompareScreen.svelte';

vi.mock('./PlannerMap.svelte', async () => ({ default: (await import('../../../test-support/planner/MapStub.svelte')).default }));

let screen: ReturnType<typeof mount> | undefined;
beforeEach(() => {
    vi.stubGlobal('fetch', vi.fn(async () => ({ ok: true, json: async () => ({ type: 'places', results: [] }) })));
});
afterEach(async () => {
    if (screen) await unmount(screen);
    screen = undefined;
    document.body.replaceChildren();
    vi.useRealTimers();
    vi.unstubAllGlobals();
});

const version = (id: string, name: string, trip: Trip): Version => ({ id, name, at: '2026-11-09T10:00:00Z', summary: '', trip });
// Six calendar days: five riding days and a rest day after day 2; and four riding days.
const long = { ...testTrip(), days: 5, restAfter: [2] }, short = { ...testTrip(), days: 4 };

function open() {
    screen = mount(CompareScreen, { target: document.body, props: {
        name: 'Basel → Thun', trip: long, line: testLine(long), openId: 'long', theme: 'light',
        versions: [version('long', 'Slow', long), version('short', 'Fast', short)],
        versionLines: { [routingKey(short)]: testLine(short) },
        onBack: () => {}, onOpen: () => {}, onFindLines: () => {},
    } });
    flushSync();
}

const column = (side: number, selector: string) => [...document.querySelectorAll<HTMLElement>(`${selector}[data-side="${side}"]`)];
const heads = () => [...document.querySelectorAll('.head')];

describe('compare screen', () => {
    it('gives each calendar day one row, and the shorter version "Finished" after its last day', () => {
        open();
        expect([...document.querySelectorAll('.rows .label')].map(label => label.textContent)).toEqual(['Day 1', 'Day 2', 'Day 3', 'Day 4', 'Day 5', 'Day 6']);
        expect(column(0, '.rest')).toHaveLength(1);
        expect(column(0, '.ride')).toHaveLength(5);
        expect(column(1, '.ride')).toHaveLength(4);
        expect(column(1, '.finished').map(cell => cell.textContent)).toEqual(['Finished', 'Finished']);
        expect(column(0, '.ride').at(-1)!.textContent).toContain('→ Thun');
    });

    it('keeps the focus while the pointer moves from one day to the next, and clears it after the pointer leaves', () => {
        vi.useFakeTimers();
        open();
        const [first, second] = column(1, '.ride');
        first.dispatchEvent(new Event('pointerenter')); flushSync();
        expect(heads()[1].classList).toContain('focused');
        expect(first.classList).toContain('marked');

        first.dispatchEvent(new Event('pointerleave'));
        vi.advanceTimersByTime(100);
        second.dispatchEvent(new Event('pointerenter'));
        vi.advanceTimersByTime(200); flushSync();
        expect(heads()[1].classList).toContain('focused');
        expect(second.classList).toContain('marked');

        second.dispatchEvent(new Event('pointerleave'));
        vi.advanceTimersByTime(200); flushSync();
        expect(heads()[1].classList).not.toContain('focused');
    });
});
