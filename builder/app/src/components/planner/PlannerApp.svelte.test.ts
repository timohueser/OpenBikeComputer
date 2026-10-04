// @vitest-environment happy-dom
import { mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { coordinateAt, cumulative, emptyTrip, initialTrip, maxRidingDays, routeCoordinates, routingKey, type Coordinate, type Place } from '../../lib/planner/editor';
import { corridorPlaces } from '../../lib/planner/place-index';
import PlannerApp from './PlannerApp.svelte';
import * as routing from '../../lib/planner/routing';
import { newPlan, type Plan } from '../../lib/planner/library';
import { storedTrip } from '../../lib/planner/trip-validation';
import type { EngineRoute, RoutingLine } from '../../lib/planner/routing';

import { routeService } from '../../../test-support/planner/route-service';
vi.mock('./PlannerMap.svelte', async () => ({ default: (await import('../../../test-support/planner/MapStub.svelte')).default }));
vi.mock('../../lib/planner/place-index', async original => ({ ...await original<object>(), corridorPlaces: vi.fn() }));

let app: ReturnType<typeof mount> | undefined;
const stored = new Map<string, string>();
let savedPlan: Plan | undefined;

vi.mock('../../lib/planner/library', async original => ({
    ...await original<object>(),
    PlanLibrary: class {
        async active() {
            const trip = storedTrip(JSON.parse(stored.get('trip') ?? 'null'));
            if (!trip && stored.has('trip')) throw new Error('Saved plan is invalid. Open another plan or import a backup.');
            return trip ? { ...newPlan(trip), revision: 1, versions: savedPlan?.versions ?? [] } : undefined;
        }
        async save(plan: Plan) {
            stored.set('trip', JSON.stringify(plan.trip));
            savedPlan = { ...plan, revision: plan.revision + 1 };
            return savedPlan;
        }
        async activate(id: string | null) { if (!id) { stored.set('trip', JSON.stringify(emptyTrip())); savedPlan = undefined; } }
        async close() {}
    },
}));
const tilePlace: Place = { id: 'poi-123', kind: 'place', label: 'Tile camp', category: 'camp', description: 'Campsite', progress: .25, coordinate: coordinateAt(routeCoordinates(initialTrip()), .25) };

beforeEach(() => {
    stored.clear();
    savedPlan = undefined;
    vi.stubGlobal('confirm', vi.fn(() => true));
    const trip = { ...initialTrip(), live: true };
    const coordinates = routeCoordinates(initialTrip());
    const distance = cumulative(coordinates);
    const line: RoutingLine = {
        key: routingKey(trip), choiceId: 'saved-route', profile: 'touring', coordinates,
        elevation: coordinates.map(() => 200), elapsed: distance.map(km => km * 240),
        edges: { pushing: coordinates.slice(1).map(() => false) }, seconds: distance.at(-1)! * 240,
        stops: [{ id: 'start', distance: 0 }, { id: 'finish', distance: distance.at(-1)! }],
        alternatives: [], alternativesReady: true, unknownSurfaceKm: 0, pushingKm: 0, unroutedKm: 0,
    };
    stored.set('trip', JSON.stringify(trip));
    vi.spyOn(routing, 'calculateLine').mockImplementation(async plan => ({ ...line, key: routingKey(plan) }));
    vi.stubGlobal('fetch', vi.fn(async (_url: string, init: RequestInit) => {
        if (!_url.includes('planner-search')) throw new Error('Routing service offline');
        const input = JSON.parse(String(init.body));
        return {ok:true,json:async () => ({type:'places',request:input.request ?? {type:'places',what:['campsite']},results:[{source:tilePlace.id,name:tilePlace.label,kind:'campsite',lon:tilePlace.coordinate[0],lat:tilePlace.coordinate[1],precision:'place',distance:0}]})};
    }));
    vi.stubGlobal('localStorage', { getItem: (key: string) => stored.get(key) ?? null, setItem: (key: string, value: string) => stored.set(key, value) });
    vi.mocked(corridorPlaces).mockResolvedValue([tilePlace]);
});

afterEach(async () => {
    if (app) await unmount(app!);
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

async function startApp() {
    app = mount(PlannerApp, { target: document.body });
    await vi.waitFor(() => expect(document.querySelector('main')?.getAttribute('aria-busy')).toBe('false'));
    await tick();
}

async function search(text: string, label = tilePlace.label) {
    const input = document.querySelector<HTMLInputElement>('[aria-label="Find a place or ask about the route"]')!;
    input.value = text;
    input.dispatchEvent(new Event('input', { bubbles: true }));
    input.form!.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }));
    await vi.waitFor(() => expect(document.querySelector('.results')?.textContent).toContain(label));
}

describe('planner app transitions', () => {
    it('uses walking access and language for a hiking route', async () => {
        const saved = JSON.parse(stored.get('trip')!);
        saved.bike = 'hiking';
        stored.set('trip', JSON.stringify(saved));
        await startApp(); await tick();
        expect(button('Pan map').dataset.accessMode).toBe('walking');
        await vi.waitFor(() => expect(document.querySelector('.trip-summary')?.textContent).toContain('Walking time'));
        const surface = document.querySelector<HTMLElement>('[aria-label="Surface along route"]')!;
        surface.focus(); await tick();
        expect(surface.getAttribute('aria-valuetext')).toContain('Walking');
        expect(document.querySelector('.push-track')).toBeNull();
        button('Map settings').click(); await tick();
        expect(document.querySelector('.layer-menu')?.textContent).toContain('Walking access.');
    });

    it('names map visits by address without adding an Undo step, and restores names on Redo', async () => {
        const saved = { ...JSON.parse(stored.get('trip')!), mode: 'route' };
        stored.set('trip', JSON.stringify(saved));
        let finish!: (response: unknown) => void;
        const original = fetch;
        vi.stubGlobal('fetch', vi.fn((url: string, init: RequestInit) => url.endsWith('/reverse')
            ? new Promise(resolve => finish = resolve) : original(url, init)));
        await startApp(); await tick();
        button('Pick map location').click(); await tick();
        button('Visit').click(); await tick();
        expect(JSON.parse(stored.get('trip')!).points).toContainEqual(expect.objectContaining({ kind: 'waypoint', label: '48.00000, 7.84000' }));
        const response = { ok: true, json: async () => ({ label: 'Dorfstraße 12, Teningen' }) };
        finish(response);
        await vi.waitFor(() => expect(document.querySelector('.route')?.textContent).toContain('Dorfstraße 12, Teningen'));
        button('Undo').click(); await tick();
        expect(JSON.parse(stored.get('trip')!)).toEqual(saved);
        button('Redo').click(); await tick();
        finish(response);
        await vi.waitFor(() => expect(JSON.parse(stored.get('trip')!).points).toContainEqual(expect.objectContaining({ label: 'Dorfstraße 12, Teningen' })));
    });

    it.each(['rename', 'remove', 'leave'] as const)('ignores an address response after %s', async action => {
        let finish!: (response: unknown) => void;
        const original = fetch;
        vi.stubGlobal('fetch', vi.fn((url: string, init: RequestInit) => url.endsWith('/reverse')
            ? new Promise(resolve => finish = resolve) : original(url, init)));
        await startApp(); await tick();
        button('Pick map location').click(); await tick();
        button('Visit').click(); await tick();
        if (action === 'rename') {
            button('Rename point').click(); await tick();
            const input = document.querySelector<HTMLInputElement>('[aria-label="Point name"]')!;
            input.value = 'Picnic'; input.dispatchEvent(new Event('change', { bubbles: true })); await tick();
        } else if (action === 'remove') { button('Remove point').click(); await tick(); }
        else { await unmount(app!); app = undefined; }
        const snapshot = stored.get('trip');
        finish({ ok: true, json: async () => ({ label: 'Late address' }) });
        await tick(); await tick();
        expect(stored.get('trip')).toBe(snapshot);
    });

    it('keeps a resolved address on reload when search is unavailable', async () => {
        const saved = JSON.parse(stored.get('trip')!);
        saved.points.push({ id: 'visit', kind: 'waypoint', autoLabel: true, label: 'Dorfstraße 12, Teningen', coordinate: [7.84, 48], progress: .5 });
        stored.set('trip', JSON.stringify(saved));
        vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new Error('Search offline')));
        await startApp(); await tick();
        expect(button('Map point: Dorfstraße 12, Teningen')).toBeDefined();
        expect(vi.mocked(fetch).mock.calls.some(([url]) => String(url).endsWith('/reverse'))).toBe(false);
    });

    it('offers any mapped place as an overnight beyond the suggestion area, and keeps the trip intact', async () => {
        const saved = { ...JSON.parse(stored.get('trip')!), mode: 'trip' };
        stored.set('trip', JSON.stringify(saved));
        vi.stubGlobal('fetch', vi.fn(async () => ({ ok: true, json: async () => ({ type: 'places', request: { type: 'place', name: 'Fuxxbau' }, results: [
            { source: 'fuxxbau', name: 'Fuxxbau', kind: 'restaurant', lon: 8.088, lat: 48.279, precision: 'place', distance: 0 },
        ] }) })));
        await startApp(); await tick();
        button('Open day 1: Basel to Overnight to choose').click(); await tick();
        await search('Fuxxbau', 'Fuxxbau');
        button('Map place: Fuxxbau').click(); await tick();
        const select = document.querySelector<HTMLSelectElement>('.callout select')!;
        expect(select.value).toBe('1');
        button('Stay here').click(); await tick();
        const draft = JSON.parse(stored.get('trip')!);
        expect(draft).toMatchObject({ mode: 'trip', days: saved.days, target: saved.target });
        expect(draft.points).toContainEqual(expect.objectContaining({ kind: 'night', night: 1, label: 'Fuxxbau', coordinate: [8.088, 48.279] }));
        expect(draft.points.find((p: { kind: string }) => p.kind === 'finish')).toEqual(saved.points.find((p: { kind: string }) => p.kind === 'finish'));
        button('Undo').click(); await tick();
        expect(JSON.parse(stored.get('trip')!)).toEqual(saved);
    });

    it('runs an open search again after a plan edit', async () => {
        await startApp(); await tick();
        await search('campsites');
        const sent = () => vi.mocked(fetch).mock.calls.map(([, init]) => JSON.parse(String(init?.body))).filter(body => body.q === 'campsites');
        const before = sent().length;
        button('Map point: Basel').click(); await tick();
        button('Remove point').click(); await tick();
        await vi.waitFor(() => expect(sent().length).toBe(before + 1));
        expect(sent().at(-1).plan.points.map((p: { label: string }) => p.label)).not.toContain('Basel');
    });

    it('sends no overnight query when the map pans', async () => {
        stored.set('trip', JSON.stringify({ ...JSON.parse(stored.get('trip')!), mode: 'trip' }));
        await startApp(); await tick();
        const overnight = () => vi.mocked(fetch).mock.calls.filter(([, init]) => JSON.parse(String(init?.body)).q === 'sleep').length;
        button('Open day 1: Basel to Overnight to choose').click(); await tick();
        await vi.waitFor(() => expect(overnight()).toBe(1));
        button('Pan map').click(); await tick();
        button('Pan map').click(); await tick();
        expect(overnight()).toBe(1);
    });

    it('links row and map hover in both directions and searches the map after point inspection', async () => {
        const saved = { ...JSON.parse(stored.get('trip')!), mode: 'route' };
        stored.set('trip', JSON.stringify(saved));
        await startApp(); await tick();
        const row = document.querySelector<HTMLButtonElement>('.route .stop')!;
        const pin = button('Map point: Basel');
        row.dispatchEvent(new MouseEvent('mouseenter')); await tick();
        expect(pin.classList.contains('highlighted')).toBe(true);
        row.dispatchEvent(new MouseEvent('mouseleave')); await tick();
        pin.dispatchEvent(new MouseEvent('mouseenter')); await tick();
        expect(row.classList.contains('highlighted')).toBe(true);
        pin.dispatchEvent(new MouseEvent('mouseleave')); await tick();
        expect(row.classList.contains('highlighted')).toBe(false);
        pin.click(); await tick();
        await search('Döner');
        const request = vi.mocked(fetch).mock.calls.find(([, init]) => JSON.parse(String(init?.body)).q === 'Döner');
        expect(JSON.parse(String(request?.[1]?.body)).pointing).toBeUndefined();
        expect(document.querySelector('.meaning')?.textContent).toContain('In this map view');
    });
    it.each(['route', 'trip'] as const)('creates and clears a %s without routing incomplete drafts', async mode => {
        stored.set('trip', JSON.stringify(emptyTrip(mode)));
        const calculate = vi.spyOn(routing, 'calculateLine').mockRejectedValue(new Error('Routing service offline'));
        await startApp();
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
        button(mode === 'route' ? 'New route' : 'New trip').click();
        await vi.waitFor(() => expect(document.body.textContent).toContain('Where would you like to ride?'));
        expect(window.confirm).not.toHaveBeenCalled();
        expect(document.body.textContent).toContain('Where would you like to ride?');
        expect(button('Undo').disabled).toBe(true);
        await unmount(app!);
        await startApp();
        expect(document.body.textContent).toContain('Where would you like to ride?');
        expect(calculate).toHaveBeenCalledTimes(2);
    });

    it('uses a search result as the first endpoint', async () => {
        stored.clear();
    savedPlan = undefined;
        const calculate = vi.spyOn(routing, 'calculateLine');
        await startApp(); await tick();
        await search('campsites');
        button(`Map place: ${tilePlace.label}`).click(); await tick();
        button('Finish here').click(); await tick();
        expect(document.body.textContent).toContain('Choose your start');
        const draft = JSON.parse(stored.get('trip')!);
        expect(draft.points).toEqual([expect.objectContaining({ kind: 'finish', label: tilePlace.label, coordinate: tilePlace.coordinate })]);
        expect(calculate).not.toHaveBeenCalled();
    });

    it('ignores an in-flight route after an endpoint is removed', async () => {
        const saved = JSON.parse(stored.get('trip')!);
        let finish!: (line: RoutingLine) => void;
        const calculate = vi.spyOn(routing, 'calculateLine').mockImplementation(() => new Promise(resolve => finish = resolve));
        await startApp(); await tick();
        button('Map point: Basel').click(); await tick();
        button('Remove point').click(); await tick();
        expect(calculate.mock.calls[0][1].aborted).toBe(true);
        finish({ key: routingKey(saved) } as RoutingLine); await tick();
        expect(document.body.textContent).toContain('Choose your start');
    });

    it('returns to an earlier bike and undoes without a route request', async () => {
        vi.mocked(routing.calculateLine).mockRestore();
        const search = globalThis.fetch, service = routeService();
        const fetch = vi.fn((url: string, init: RequestInit) => url.endsWith('/v1/route') ? service(url, init) : search(url, init));
        vi.stubGlobal('fetch', fetch);
        const routeRequests = () => fetch.mock.calls.filter(([url]) => url.endsWith('/v1/route')).length;
        await startApp();
        for (const [bike, requests] of [['Gravel bike', 1], ['Touring bike', 2]] as const) {
            await vi.waitFor(() => expect(document.querySelector('section.elevation')).not.toBeNull());
            expect(routeRequests()).toBe(requests);
            button('Activity').click(); await tick();
            button(bike).click(); await tick();
        }
        for (const action of ['Undo', 'Undo', 'Redo']) {
            button(action).click(); await tick();
            await vi.waitFor(() => expect(document.querySelector('section.elevation')).not.toBeNull());
        }
        expect(routeRequests()).toBe(2);
    });

    it('keeps a picked corridor, without its alternatives, through undo, redo, a saved version and a reload', async () => {
        const engine = (id: string, reason: string, geometry: Coordinate[]): EngineRoute => {
            const km = cumulative(geometry).at(-1)!;
            const totals = { distance_m: km * 1000, ascent_m: 0, seconds: km * 240, surface_m: [0, km * 1000, 0, 0, 0, 0], unknown_elevation_m: 0, pushing_m: 0 };
            return { id, reason, package: 'test', profile: 'touring', geometry, elevation: geometry.map(() => 200), elapsed: cumulative(geometry).map(d => d * 240),
                edges: {}, totals,
                legs: [{ from_index: 0, to_index: geometry.length - 1, start: 'a', end: 'b', totals }], snap_truncated: false };
        };
        const coordinates = routeCoordinates(initialTrip());
        const primary = engine('primary', 'primary', coordinates);
        const corridor = engine('corridor', 'corridor', coordinates.map(([x, y], i) => (i && i < coordinates.length - 1 ? [x, y + .02] : [x, y]) as Coordinate));
        vi.mocked(routing.calculateLine).mockImplementation(async plan => routing.selectRoute(plan, primary, [primary, corridor]));
        vi.spyOn(routing, 'requestAlternatives').mockResolvedValue([primary, corridor]);
        const chosen = () => document.querySelector('.ways [aria-checked="true"]')?.textContent ?? '';
        const options = async () => { await vi.waitFor(() => button('Route options · 2').click()); await tick(); };
        await startApp();
        await options();
        [...document.querySelectorAll<HTMLButtonElement>('.ways [role="radio"]')].find(way => way.textContent?.includes('Different corridor'))!.click(); await tick();
        expect(chosen()).toContain('Different corridor');
        expect(JSON.parse(stored.get('trip')!).routing).toMatchObject({ choiceId: 'corridor', picked: true, alternatives: [] });
        button('Undo').click(); await tick();
        await vi.waitFor(() => expect(chosen()).toContain('Balanced'));
        button('Redo').click(); await tick();
        expect(chosen()).toContain('Different corridor');
        button('Save version').click(); await tick();
        document.querySelector('form.name')!.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }));
        await vi.waitFor(() => expect(document.querySelector('form.name')).toBeNull());
        button('Undo').click(); await tick();
        await vi.waitFor(() => expect(chosen()).toContain('Balanced'));
        button('Restore').click(); await tick();
        await vi.waitFor(() => expect(chosen()).toContain('Different corridor'));
        await unmount(app!);
        const calculated = vi.mocked(routing.calculateLine).mock.calls.length;
        await startApp(); await tick();
        expect(routing.calculateLine).toHaveBeenCalledTimes(calculated);
        expect(routing.requestAlternatives).toHaveBeenCalledTimes(1);
        button('Route options').click();
        await vi.waitFor(() => expect(chosen()).toContain('Different corridor'));
        for (const bike of ['Gravel bike', 'Touring bike']) {
            button('Activity').click(); await tick();
            button(bike).click(); await tick();
            await vi.waitFor(() => expect(chosen()).not.toBe(''));
        }
        expect(chosen()).toContain('Balanced');
        button('Undo').click(); await tick();
        button('Undo').click(); await tick();
        expect(chosen()).toContain('Different corridor');
    });

    it('opens a tile place from both a search row and its map pin, then adds a visit', async () => {
        await startApp();
        await tick();
        await search('campsites');
        const request = vi.mocked(fetch).mock.calls.find(([, init]) => JSON.parse(String(init?.body)).q === 'campsites');
        const plan = JSON.parse(String(request?.[1]?.body)).plan;
        expect(plan.hours).toHaveLength(plan.coordinates.length);
        expect(plan.hours.at(-1)).toBeGreaterThan(0);
        const row = [...document.querySelectorAll<HTMLButtonElement>('.results button')].find(button => button.textContent?.includes(tilePlace.label))!;
        expect(row).toBeDefined();
        const pin = button(`Map place: ${tilePlace.label}`);
        row.dispatchEvent(new MouseEvent('mouseenter')); await tick();
        expect(pin.classList.contains('highlighted')).toBe(true);
        row.dispatchEvent(new MouseEvent('mouseleave')); await tick();
        expect(pin.classList.contains('highlighted')).toBe(false);
        pin.dispatchEvent(new MouseEvent('mouseenter')); await tick();
        expect(row.classList.contains('hovered')).toBe(true);
        pin.dispatchEvent(new MouseEvent('mouseleave')); await tick();
        expect(row.classList.contains('hovered')).toBe(false);
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
        await startApp();
        await tick();
        button('Open day 1: Basel to Overnight to choose').click();
        await tick();
        await vi.waitFor(() => expect(document.querySelector('.choose')?.textContent).toContain(tilePlace.label));
        const choices = document.querySelector('.choose')!.textContent;
        expect(choices).not.toContain('≈ 0.0 km');
        const profile = document.querySelector('.elevation');
        const requests = vi.mocked(fetch).mock.calls.length;
        button('Pan map').click(); await tick();
        expect(fetch).toHaveBeenCalledTimes(requests);
        expect(document.querySelector('.choose')!.textContent).toBe(choices);
        button('All days').click(); await tick();
        expect(document.querySelector('.elevation')).toBe(profile);
        button('Open day 1: Basel to Overnight to choose').click(); await tick();
        expect(fetch).toHaveBeenCalledTimes(requests);
        await search('campsites day 2');
        button('Clear search').click();
        await tick();
        expect(document.querySelector<HTMLSelectElement>('[aria-label="Selected day"]')?.value).toBe('1');
        const day = document.querySelector('.day')!;
        expect(day.querySelector('.choose')!.textContent).toBe(choices);
        expect(day.textContent).not.toContain('Meadow camp');
    });

    it('keeps the profile mounted while an overnight reroutes and when the reply arrives', async () => {
        const measured = vi.mocked(routing.calculateLine).getMockImplementation()!;
        let line!: RoutingLine, finish!: (line: RoutingLine) => void;
        const calculate = vi.mocked(routing.calculateLine)
            .mockImplementationOnce(async (...args) => line = await measured(...args))
            .mockImplementation(() => new Promise(resolve => finish = resolve));
        await startApp();
        await vi.waitFor(() => button('Open day 1: Basel to Overnight to choose').click()); await tick();
        await vi.waitFor(() => expect(document.querySelector('.choose')?.textContent).toContain(tilePlace.label));
        const profile = document.querySelector('.elevation');
        button(`Map place: ${tilePlace.label}`).click(); await tick();
        button('Stay here').click(); await tick();
        expect(document.querySelector('.elevation')).toBe(profile);
        expect(document.querySelector('.route-status')?.textContent).toContain('Calculating route');
        button('Pan map').click(); await tick();
        expect(calculate).toHaveBeenCalledTimes(2);
        const draft = JSON.parse(stored.get('trip')!);
        finish({ ...line, key: routingKey(draft), stops: [line.stops[0], { id: 'night-1', distance: line.stops[1].distance * .25 }, line.stops[1]] });
        await vi.waitFor(() => expect(document.querySelector('.route-status')).toBeNull());
        expect(document.querySelector('.elevation')).toBe(profile);
        expect(document.querySelector('.day h2')?.textContent).toContain(tilePlace.label);
    });

    it('rejects a malformed cached draft without crashing the planner', async () => {
        stored.set('trip', JSON.stringify({ points: [], days: 3, limit: 50 }));
        await startApp();
        await tick();
        expect(document.body.textContent).toContain('Saved plan is invalid.');
        expect(document.body.textContent).toContain('Where would you like to ride?');
        expect(document.body.textContent).toContain('Choose a start and finish to get started.');
    });

    it('prevents adding an overnight past the riding-day limit', async () => {
        await startApp();
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
