// @vitest-environment happy-dom
import { mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { coordinateAt, cumulative, emptyTrip, initialTrip, maxRidingDays, routeCoordinates, routingKey, type Place } from '../../lib/planner/editor';
import { corridorPlaces } from '../../lib/planner/place-index';
import PlannerApp from './PlannerApp.svelte';
import * as routing from '../../lib/planner/routing';
import type { RoutingLine } from '../../lib/planner/routing';

vi.mock('./PlannerMap.svelte', async () => ({ default: (await import('../../../test-support/planner/MapStub.svelte')).default }));
vi.mock('../../lib/planner/place-index', () => ({ corridorPlaces: vi.fn() }));

let app: ReturnType<typeof mount>;
const stored = new Map<string, string>();
const tilePlace: Place = { id: 'poi-123', kind: 'place', label: 'Tile camp', category: 'camp', description: 'Campsite', progress: .25, coordinate: coordinateAt(routeCoordinates(initialTrip()), .25) };

beforeEach(() => {
    stored.clear();
    const trip = { ...initialTrip(), live: true };
    const coordinates = routeCoordinates(initialTrip());
    const distance = cumulative(coordinates);
    const line: RoutingLine = {
        key: routingKey(trip), choiceId: 'saved-route', profile: 'touring', coordinates,
        elevation: coordinates.map(() => 200), elapsed: distance.map(km => km * 240),
        surfaces: coordinates.slice(1).map(() => 'Paved'), seconds: distance.at(-1)! * 240,
        stops: [{ id: 'start', distance: 0 }, { id: 'finish', distance: distance.at(-1)! }],
        alternatives: [], alternativesReady: true, unknownSurfaceKm: 0, pushingKm: 0, unroutedKm: 0,
    };
    stored.set('obc-planner-routing-v2', JSON.stringify({ ...trip, routing: line }));
    vi.stubGlobal('fetch', vi.fn(async (_url: string, init: RequestInit) => {
        if (!_url.includes('planner-search')) throw new Error('Routing service offline');
        const input = JSON.parse(String(init.body));
        return {ok:true,json:async () => ({type:'places',request:input.request ?? {type:'places',what:['campsite']},results:[{source:tilePlace.id,name:tilePlace.label,kind:'campsite',lon:tilePlace.coordinate[0],lat:tilePlace.coordinate[1],precision:'place',distance:0}]})};
    }));
    vi.stubGlobal('localStorage', { getItem: (key: string) => stored.get(key) ?? null, setItem: (key: string, value: string) => stored.set(key, value) });
    vi.mocked(corridorPlaces).mockResolvedValue([tilePlace]);
});

afterEach(async () => {
    if (app) await unmount(app);
    vi.useRealTimers();
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
    const input = document.querySelector<HTMLInputElement>('[aria-label="Find a place or ask about the route"]')!;
    input.value = text;
    input.dispatchEvent(new Event('input', { bubbles: true }));
    input.form!.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }));
    await vi.waitFor(() => expect(document.querySelector('.results')?.textContent).toContain(tilePlace.label));
}

describe('planner app transitions', () => {
    it.each(['route', 'trip'] as const)('creates and clears a %s without routing incomplete drafts', async mode => {
        stored.set('obc-planner-routing-v2', JSON.stringify(emptyTrip(mode)));
        const calculate = vi.spyOn(routing, 'calculateLine').mockRejectedValue(new Error('Routing service offline'));
        app = mount(PlannerApp, { target: document.body });
        await tick();
        expect(document.body.textContent).toContain('Where would you like to ride?');
        expect(calculate).not.toHaveBeenCalled();
        button('Pick map location').click(); await tick();
        button('Finish here').click(); await tick();
        expect(document.body.textContent).toContain('Choose your start');
        expect(calculate).not.toHaveBeenCalled();
        button('Pick map location').click(); await tick();
        button('Start here').click(); await tick();
        await vi.waitFor(() => expect(calculate).toHaveBeenCalledTimes(1));
        expect(document.body.textContent).toContain('Routing service offline');
        button('Retry routing').click(); await tick();
        await vi.waitFor(() => expect(calculate).toHaveBeenCalledTimes(2));
        button('Map point: Start').click(); await tick();
        button('Remove point').click(); await tick();
        expect(document.body.textContent).toContain('Choose your start');
        expect(calculate).toHaveBeenCalledTimes(2);
        button('Map point: Finish').click(); await tick();
        button('Remove point').click(); await tick();
        expect(document.body.textContent).toContain('Where would you like to ride?');
        button('Undo').click(); await tick();
        expect(document.body.textContent).toContain('Choose your start');
        button(mode === 'route' ? 'New route' : 'New trip').click(); await tick();
        expect(document.body.textContent).toContain('Where would you like to ride?');
        button('Undo').click(); await tick();
        await unmount(app);
        app = mount(PlannerApp, { target: document.body }); await tick();
        expect(document.body.textContent).toContain('Choose your start');
        expect(calculate).toHaveBeenCalledTimes(2);
    });

    it('uses a search result as the first endpoint', async () => {
        stored.clear();
        const calculate = vi.spyOn(routing, 'calculateLine');
        app = mount(PlannerApp, { target: document.body }); await tick();
        await search('campsites');
        button(`Map place: ${tilePlace.label}`).click(); await tick();
        button('Finish here').click(); await tick();
        expect(document.body.textContent).toContain('Choose your start');
        const draft = JSON.parse(stored.get('obc-planner-routing-v2')!);
        expect(draft.points).toEqual([expect.objectContaining({ kind: 'finish', label: tilePlace.label, coordinate: tilePlace.coordinate })]);
        expect(calculate).not.toHaveBeenCalled();
    });

    it('ignores an in-flight route after an endpoint is removed', async () => {
        const saved = JSON.parse(stored.get('obc-planner-routing-v2')!);
        delete saved.routing;
        stored.set('obc-planner-routing-v2', JSON.stringify(saved));
        let finish!: (line: RoutingLine) => void;
        const calculate = vi.spyOn(routing, 'calculateLine').mockImplementation(() => new Promise(resolve => finish = resolve));
        app = mount(PlannerApp, { target: document.body }); await tick();
        button('Map point: Basel').click(); await tick();
        button('Remove point').click(); await tick();
        expect(calculate.mock.calls[0][1].aborted).toBe(true);
        finish({ key: routingKey(saved) } as RoutingLine); await tick();
        expect(document.body.textContent).toContain('Choose your start');
        expect(JSON.parse(stored.get('obc-planner-routing-v2')!).routing).toBeUndefined();
    });

    it('opens a tile place from both a search row and its map pin, then adds a visit', async () => {
        app = mount(PlannerApp, { target: document.body });
        await tick();
        await search('campsites');
        const request = vi.mocked(fetch).mock.calls.find(([, init]) => JSON.parse(String(init?.body)).q === 'campsites');
        const plan = JSON.parse(String(request?.[1]?.body)).plan;
        expect(plan.hours).toHaveLength(plan.coordinates.length);
        expect(plan.hours.at(-1)).toBeGreaterThan(0);
        const row = [...document.querySelectorAll<HTMLButtonElement>('.results button')].find(button => button.textContent?.includes(tilePlace.label))!;
        expect(row).toBeDefined();
        const searches = vi.mocked(fetch).mock.calls.length;
        vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
        row.click();
        await tick();
        await vi.advanceTimersByTimeAsync(1000);
        expect(fetch).toHaveBeenCalledTimes(searches);
        expect(row.isConnected).toBe(true);
        expect(document.querySelector('.results')?.getAttribute('aria-busy')).toBe('false');
        expect(document.querySelector('[role="dialog"] h2')?.textContent).toBe(tilePlace.label);
        button('Close').click();
        await tick();
        await vi.waitFor(() => expect(button(`Map place: ${tilePlace.label}`)).toBeDefined());
        button(`Map place: ${tilePlace.label}`).click();
        await tick();
        await vi.advanceTimersByTimeAsync(1000);
        expect(fetch).toHaveBeenCalledTimes(searches);
        expect(row.isConnected).toBe(true);
        expect(document.querySelector('[role="dialog"] h2')?.textContent).toBe(tilePlace.label);
        vi.useRealTimers();
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
        expect(document.body.textContent).toContain('Saved draft is invalid · new plan opened');
        expect(document.body.textContent).toContain('Where would you like to ride?');
        expect(document.body.textContent).toContain('Choose a start and finish to get started.');
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
        button(`Open day ${maxRidingDays}: Day ${maxRidingDays - 1} overnight to Besançon`).click();
        await tick();
        await search(`campsites day ${maxRidingDays}`);
        await vi.waitFor(() => expect(button(`Map place: ${tilePlace.label}`)).toBeDefined());
        button(`Map place: ${tilePlace.label}`).click();
        await tick();
        const dialog = document.querySelector('[role="dialog"]')!;
        expect(button('Stay here', dialog).disabled).toBe(true);
        expect(dialog.querySelector<HTMLOptionElement>(`option[value="${maxRidingDays}"]`)?.disabled).toBe(true);
        expect(dialog.textContent).toContain(`up to ${maxRidingDays} riding days`);
    });
});
