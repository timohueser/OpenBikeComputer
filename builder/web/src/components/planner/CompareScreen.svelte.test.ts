// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Trip } from '../../lib/planner/editor';
import { routingKey } from '../../lib/planner/routing';
import { newPlan, type Plan } from '../../lib/planner/library';
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

const route = (id: string, name: string, trip: Trip): Plan => ({ ...newPlan(trip, name), id });
// Six calendar days: five riding days and a rest day after day 2; four riding days; and three.
const long = { ...testTrip(), days: 5, restAfter: [2] }, short = { ...testTrip(), days: 4 }, shortest = { ...testTrip(), days: 3 };
const lines = Object.fromEntries([long, short, shortest].map(trip => [routingKey(trip), testLine(trip)]));

function open(routes = [route('short', 'Fast', short), route('long', 'Slow', long)], props: { changed?: boolean; onFindLines?: (trips: Trip[]) => void } = {}) {
    screen = mount(CompareScreen, { target: document.body, props: {
        routes, first: 'long', changed: false, lines, theme: 'light', onBack: () => {}, onOpen: () => {}, onFindLines: () => {}, ...props,
    } });
    flushSync();
}

const column = (side: number, selector: string) => [...document.querySelectorAll<HTMLElement>(`${selector}[data-side="${side}"]`)];
const heads = () => [...document.querySelectorAll('.head')];

describe('compare screen', () => {
    it('starts on the open route and the newest other route, and asks for the lines of these two only', () => {
        const onFindLines = vi.fn();
        open([route('shortest', 'Hills', shortest), route('short', 'Fast', short), route('long', 'Slow', long)], { onFindLines });
        expect([...document.querySelectorAll('.head .name strong')].map(name => name.textContent)).toEqual(['Slow', 'Hills']);
        expect(onFindLines).toHaveBeenLastCalledWith([long, shortest]);
        expect(document.querySelector('.note')).toBeNull();
    });

    it('asks for a second saved route, and says when the open route shows its saved state', async () => {
        open([route('long', 'Slow', long)]);
        expect(document.querySelector('.empty')?.textContent).toBe('Save two routes to compare them.');
        expect(document.querySelector('[aria-label="First route"]')).toBeNull();
        await unmount(screen!);
        open([], { changed: true });
        expect(document.querySelector('.empty')?.textContent).toBe('Save two routes to compare them.');
        await unmount(screen!);
        open(undefined, { changed: true });
        expect(document.querySelector('.note')?.textContent).toBe('‘Slow’ shows its saved state. Your latest changes could not be saved.');
    });

    it('gives each calendar day one row, and the shorter route "Finished" after its last day', () => {
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
        vi.advanceTimersByTime(100); flushSync();
        expect(heads()[1].classList).toContain('focused');
        second.dispatchEvent(new Event('pointerenter'));
        vi.advanceTimersByTime(200); flushSync();
        expect(heads()[1].classList).toContain('focused');
        expect(second.classList).toContain('marked');

        second.dispatchEvent(new Event('pointerleave'));
        vi.advanceTimersByTime(200); flushSync();
        expect(heads()[1].classList).not.toContain('focused');
    });
});
