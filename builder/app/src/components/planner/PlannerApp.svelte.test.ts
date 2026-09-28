// @vitest-environment happy-dom
import { mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { coordinateAt, initialTrip, maxRidingDays, routeCoordinates, type Place } from '../../lib/planner/editor';
import { corridorPlaces } from '../../lib/planner/place-index';
import PlannerApp from './PlannerApp.svelte';

vi.mock('./PlannerMap.svelte', async () => ({ default: (await import('../../../test-support/planner/MapStub.svelte')).default }));
vi.mock('../../lib/planner/place-index', () => ({ corridorPlaces: vi.fn() }));

let app: ReturnType<typeof mount>;
const stored = new Map<string, string>();
const tilePlace: Place = { id: 'poi-123', kind: 'place', label: 'Tile camp', category: 'camp', description: 'Campsite', progress: .25, coordinate: coordinateAt(routeCoordinates(initialTrip()), .25) };

beforeEach(() => {
    stored.clear();
    vi.stubGlobal('localStorage', { getItem: (key: string) => stored.get(key) ?? null, setItem: (key: string, value: string) => stored.set(key, value) });
    vi.mocked(corridorPlaces).mockResolvedValue([tilePlace]);
});

afterEach(async () => {
    if (app) await unmount(app);
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
    document.body.replaceChildren();
});

function button(label: string, scope: ParentNode = document) {
    const found = [...scope.querySelectorAll('button')].find(button => (button.getAttribute('aria-label') ?? button.textContent?.trim()) === label);
    if (!found) throw new Error(`Missing button: ${label}`);
    return found;
}

async function search(text: string) {
    const input = document.querySelector<HTMLInputElement>('[aria-label="Search places along your route"]')!;
    input.value = text;
    input.dispatchEvent(new Event('input', { bubbles: true }));
    input.form!.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }));
    await tick();
    await tick();
}

describe('planner app transitions', () => {
    it('opens a tile place from both a search row and its map pin, then adds a visit', async () => {
        app = mount(PlannerApp, { target: document.body });
        await tick();
        await search('campsites');
        const row = [...document.querySelectorAll<HTMLButtonElement>('.results button')].find(button => button.textContent?.includes(tilePlace.label))!;
        expect(row).toBeDefined();
        row.click();
        await tick();
        expect(document.querySelector('[role="dialog"] h2')?.textContent).toBe(tilePlace.label);
        button('Close').click();
        await tick();
        button(`Map place: ${tilePlace.label}`).click();
        await tick();
        expect(document.querySelector('[role="dialog"] h2')?.textContent).toBe(tilePlace.label);
        button('Add as visit').click();
        await tick();
        const draft = [...stored.values()].map(value => JSON.parse(value)).find(value => value.points);
        expect(draft.points).toContainEqual(expect.objectContaining({ label: tilePlace.label, kind: 'waypoint' }));
    });

    it('restores the focused day after a different-day search', async () => {
        app = mount(PlannerApp, { target: document.body });
        await tick();
        button('Open day 1: Basel to Overnight to choose').click();
        await tick();
        const choices = document.querySelector('.choose')!.textContent;
        await search('campsites day 2');
        button('Clear search').click();
        await tick();
        expect(document.querySelector<HTMLSelectElement>('[aria-label="Selected day"]')?.value).toBe('1');
        const day = document.querySelector('.day')!;
        expect(day.querySelector('.choose')!.textContent).toBe(choices);
        expect(day.textContent).not.toContain('Meadow camp');
    });

    it('rejects a malformed cached draft without crashing the planner', async () => {
        vi.spyOn(localStorage, 'getItem').mockReturnValueOnce(JSON.stringify({ points: [], days: 3, limit: 50 }));
        app = mount(PlannerApp, { target: document.body });
        await tick();
        expect(document.body.textContent).toContain('Saved draft is invalid · example route loaded');
        expect(button('Open day 1: Basel to Overnight to choose')).toBeDefined();
    });

    it('prevents adding an overnight past the riding-day limit', async () => {
        app = mount(PlannerApp, { target: document.body });
        await tick();
        button('Edit day plan').click();
        await tick();
        const days = document.querySelector<HTMLInputElement>('[aria-label="Number of days"]')!;
        days.value = String(maxRidingDays);
        days.dispatchEvent(new Event('input', { bubbles: true }));
        days.form!.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }));
        await tick();
        await search(`campsites day ${maxRidingDays}`);
        button(`Map place: ${tilePlace.label}`).click();
        await tick();
        const dialog = document.querySelector('[role="dialog"]')!;
        expect(button('Stay here', dialog).disabled).toBe(true);
        expect(dialog.querySelector<HTMLOptionElement>(`option[value="${maxRidingDays}"]`)?.disabled).toBe(true);
        expect(dialog.textContent).toContain(`up to ${maxRidingDays} riding days`);
    });
});
