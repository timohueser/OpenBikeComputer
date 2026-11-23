// @vitest-environment happy-dom

import { mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, onTestFinished, vi } from 'vitest';
import { allStay, closeLoop, planView, type Place, type Stay, type Trip } from '../../lib/planner/editor';
import { testLine, testTrip } from '../../../test-support/planner/trip';
import type { NightChoices } from '../../lib/planner/night-stops.svelte';
import { proposal, type Stop } from '../../lib/planner/stops';
import Itinerary from './Itinerary.svelte';
import Profile from './PlannerProfile.svelte';
import MapCallout from './MapCallout.svelte';
import Select from './PlannerSelect.svelte';
import RouteList from './RouteList.svelte';
import RouteStats from './RouteStats.svelte';
import TripBar from './TripBar.svelte';
import TripRow from './TripRow.svelte';
import RoutePreferences from './RoutePreferences.svelte';
import { coordinateName } from '../../lib/planner/point-names';
import SaveWindow, { type SaveMode } from './SaveWindow.svelte';
import type { RoutingLine } from '../../lib/planner/routing';
import { settings } from '../../lib/settings/settings.svelte';

const places: Place[] = [{id:'test-camp',kind:'place',label:'Test camp',category:'camp',description:'',coordinate:[7.5,47.5]}];

const mounted: ReturnType<typeof mount>[] = [];
const saveProps = { stored: false, changed: false, canSave: false, onSave: vi.fn(), onRename: vi.fn(), more: [] };

beforeEach(() => {
    const values = new Map<string, string>();
    vi.stubGlobal('localStorage', {
        getItem: (key: string) => values.get(key) ?? null,
        setItem: (key: string, value: string) => values.set(key, value),
    });
});

afterEach(async () => {
    for (const component of mounted.splice(0)) await unmount(component);
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
    document.body.replaceChildren();
});

function button(name: string) {
    const found = [...document.querySelectorAll('button')].find(button =>
        (button.getAttribute('aria-label') ?? button.textContent?.trim()) === name);
    if (!found) throw new Error(`Missing button: ${name}`);
    return found;
}

describe('planner preferences', () => {
    it('browses without changing the value, cancels, and commits a keyboard choice', async () => {
        const onChange = vi.fn();
        mounted.push(mount(Select, { target: document.body, props: {
            label: 'Bike', value: 'touring', onChange,
            options: [{ value: 'touring', label: 'Touring bike' }, { value: 'gravel', label: 'Gravel bike' }, { value: 'road', label: 'Road bike' }],
        } }));
        await tick();
        const trigger = button('Bike');
        trigger.focus();
        const key = async (key: string) => { trigger.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true })); await tick(); };
        await key('ArrowDown');
        await key('End');
        expect(document.getElementById(trigger.getAttribute('aria-activedescendant')!)?.textContent).toContain('Road bike');
        expect(onChange).not.toHaveBeenCalled();
        await key('Escape');
        expect(trigger.getAttribute('aria-expanded')).toBe('false');
        expect(document.activeElement).toBe(trigger);
        await key('Enter');
        await key('g');
        await key('Enter');
        expect(onChange).toHaveBeenCalledExactlyOnceWith('gravel');
        expect(trigger.getAttribute('aria-expanded')).toBe('false');
        trigger.click();
        await tick();
        document.body.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }));
        await tick();
        expect(trigger.getAttribute('aria-expanded')).toBe('false');
    });
});

describe('route stop handles', () => {
    const stops = ['Start', 'First', 'Second', 'Third', 'Finish'].map((label, index) => ({
        point: { id: label, label, kind: index === 0 ? 'start' as const : index === 4 ? 'finish' as const : 'waypoint' as const, progress: index / 4, coordinate: [7, 47] as [number, number] },
        distance: index * 10,
    }));

    it('commits one move on drop and leaves a cancelled drag unchanged', async () => {
        const onReorder = vi.fn();
        const shaped = [...stops.slice(0,2), {point:{...stops[1].point,id:'shape',label:'Shaping point',kind:'via' as const},distance:15}, ...stops.slice(2)];
        mounted.push(mount(RouteList, { target: document.body, props: { stops: shaped, onReorder, onInspect: vi.fn() } }));
        await tick();
        expect(document.querySelectorAll('li')).toHaveLength(stops.length);
        expect(document.querySelector('ol')?.textContent).not.toContain('Shaping point');
        const rect = (top: number, height: number) => ({ top, bottom: top + height, left: 0, right: 360, width: 360, height, x: 0, y: top, toJSON: () => ({}) });
        vi.spyOn(document.querySelector('ol')!, 'getBoundingClientRect').mockReturnValue(rect(0, 250));
        document.querySelectorAll('li').forEach((row, index) => vi.spyOn(row, 'getBoundingClientRect').mockReturnValue(rect(index * 50, 50)));
        const handle = button('Reorder First');
        handle.setPointerCapture = vi.fn();
        const pointer = async (type: string, clientY: number) => {
            handle.dispatchEvent(new PointerEvent(type, { button: 0, pointerId: 1, clientX: 340, clientY, bubbles: true }));
            await tick();
        };
        await pointer('pointerdown', 75);
        await pointer('pointermove', 175);
        expect(onReorder).not.toHaveBeenCalled();
        await pointer('pointerup', 175);
        expect(onReorder).toHaveBeenCalledExactlyOnceWith('First', 2);
        expect(document.querySelector('[role="status"]')?.textContent).toBe('First moved to stop 3 of 3. Changed legs follow roads.');
        onReorder.mockClear();
        await pointer('pointerdown', 75);
        await pointer('pointermove', 175);
        window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
        await pointer('pointerup', 175);
        expect(onReorder).not.toHaveBeenCalled();
    });

    it('supports arrow keys without moving a stop past either endpoint', async () => {
        const onReorder = vi.fn();
        mounted.push(mount(RouteList, { target: document.body, props: { stops, onReorder, onInspect: vi.fn() } }));
        await tick();
        button('Reorder First').dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowUp', bubbles: true }));
        button('Reorder Third').dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }));
        expect(onReorder).not.toHaveBeenCalled();
        button('Reorder Second').dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowUp', bubbles: true }));
        expect(onReorder).toHaveBeenCalledExactlyOnceWith('Second', -1);
    });
});

function nightChoices(choices: Partial<NightChoices> = {}): NightChoices {
    return { choosing: null, stopId: null, shownId: null, kinds: () => allStay, found: () => undefined, window: () => null, around: () => undefined,
        campsites: 0, keepCampsites: vi.fn(),
        choose: vi.fn(), open: vi.fn(), endHere: vi.fn(), stay: vi.fn(),
        selectPlace: vi.fn(), pick: vi.fn(), retry: vi.fn(), setKinds: vi.fn(), release: vi.fn(), ...choices };
}

const town = (name: string, km: number): Stop => ({ id: name, name, km, from: km, to: km, food: [], shop: [],
    sleep: [{ place: { id: `${name}-camp`, kind: 'place', category: 'camp', placeKind: 'campsite', label: `${name} camping`, description: '', coordinate: [7.6, 47.2], locality: name }, km, off: .2 }] });

function itineraryProps(expandedDay: number | null) {
    const trip = testTrip(), line = testLine(trip);
    return {
        trip, line, expandedDay, nightPage: null as number | null, days: planView(trip, line).days, theme: 'light' as const,
        nights: nightChoices(), conflicts: [], selectedId: null, revealId: null,
        onToggle: vi.fn(), onOverview: vi.fn(), onInspect: vi.fn(), onShowEnd: vi.fn(),
        onShowConflict: vi.fn(),
    };
}

describe('planner day views', () => {
    it('keeps every day in the overview and opens a day', async () => {
        const props = itineraryProps(null);
        mounted.push(mount(Itinerary, { target: document.body, props }));
        await tick();

        expect(document.querySelectorAll('[data-day]')).toHaveLength(3);
        expect(document.body.textContent).not.toContain('Where to sleep');
        document.querySelector<HTMLButtonElement>('[data-day="2"]')!.click();
        await tick();
        expect(props.onToggle).toHaveBeenCalledWith(2);
    });

    it('lists the towns of a night on its own page, by the kinds of place of the night, and goes back one level', async () => {
        const props = itineraryProps(null), total = props.days.at(-1)!.to * planView(props.trip, props.line).total;
        const [laufen, olten, bern] = [town('Laufen', total * .2), town('Olten', total * .3), town('Bern', total * .45)];
        Object.assign(olten.sleep[0].place, { placeKind: 'hotel', category: 'hotel' });
        bern.sleep.push({ ...olten.sleep[0], km: bern.km });
        const stops = [laufen, olten, bern];
        const nights: NightChoices = $state(nightChoices({ choosing: 1, campsites: 2, kinds: () => ['camp'], around: () => proposal(bern, ['camp']),
            found: () => ({ state: 'ready', stops }), window: () => [0, total * .6], open: id => { nights.stopId = id; },
            choose: night => { nights.choosing = page = night; } }));
        let page = $state<number | null>(1);
        props.nights = nights;
        mounted.push(mount(Itinerary, { target: document.body, props: { ...props, get nightPage() { return page; } } }));
        await tick(); await tick();

        expect(document.querySelector('[data-day]')).toBeNull();
        expect(document.querySelector('.keep')).toBeNull();
        expect(document.activeElement).toBe(button('All days'));
        const rows = () => [...document.querySelectorAll<HTMLButtonElement>('ul[aria-label="Towns for night 1"] button.town')];
        const towns = rows();
        expect(towns.map(b => b.querySelector('strong')?.textContent)).toEqual(['Laufen', 'BernEven days']);
        expect(towns[1].getAttribute('aria-label')).toMatch(/^Bern, 1 campsite\. Day 1 \d+h \d\dm, day 2 \d+h \d\dm\. Even days$/);
        expect(document.querySelector('.tools')!.compareDocumentPosition(towns[0]) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
        expect(document.querySelectorAll('ul[aria-label="Towns for night 1"] li button')).toHaveLength(towns.length);

        towns[0].click(); await tick();
        expect(document.querySelector('h2')).toBeNull();
        button('Night 1').dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })); await tick();
        expect(nights.stopId).toBeNull();
        expect(document.activeElement).toBe(rows()[0]);

        button('All days').click();
        await tick(); await tick();
        expect(document.querySelectorAll('[data-day]')).toHaveLength(3);
        expect(document.querySelector('.keep')).not.toBeNull();
        expect(document.activeElement).toBe(button('Choose night 1'));
    });

    it('selects where to sleep in a town, and one button ends the day there', async () => {
        const props = itineraryProps(null), total = props.days.at(-1)!.to * planView(props.trip, props.line).total;
        const bern = town('Bern', total * .45);
        const hotel: Place = { id: 'Bern-hotel', kind: 'place', category: 'hotel', placeKind: 'hotel', label: 'Hotel Bären', description: '', coordinate: [7.6, 47.2], locality: 'Bern' };
        bern.sleep.push({ place: hotel, km: bern.km, off: 1 }, { place: { ...bern.sleep[0].place, id: 'Bern-unnamed', label: 'campsite' }, km: bern.km, off: .5 });
        let kinds = $state<Stay[]>(['camp', 'hotel']);
        const endHere = vi.fn(), stay = vi.fn(), selectPlace = vi.fn();
        props.nights = nightChoices({ choosing: 1, stopId: 'Bern', kinds: () => kinds, found: () => ({ state: 'ready', stops: [bern] }),
            window: () => [0, total * .6], endHere, stay, selectPlace });
        mounted.push(mount(Itinerary, { target: document.body, props: { ...props, nightPage: 1 } }));
        await tick();
        const confirm = () => document.querySelector<HTMLButtonElement>('.confirm button')!;
        const radio = (name: string) => [...document.querySelectorAll('label')].find(row => row.textContent?.includes(name))!.querySelector('input')!;

        expect(document.querySelector('[role="radiogroup"]')?.getAttribute('aria-label')).toBe('Where you sleep on night 1');
        expect(radio('Somewhere in Bern').checked).toBe(true);
        expect(document.body.textContent).not.toContain('Water');
        expect(confirm().textContent?.trim()).toBe('End day 1 in Bern');
        confirm().click();
        expect(endHere).toHaveBeenCalledWith(1, bern);

        radio('Hotel Bären').click(); await tick();
        expect(selectPlace).toHaveBeenCalledExactlyOnceWith(hotel);
        expect(confirm().textContent?.trim()).toBe('End day 1 at Hotel Bären');
        confirm().click();
        expect(stay).toHaveBeenCalledWith(1, hotel);
        // A click on the checked place shows it on the map again.
        radio('Hotel Bären').click(); await tick();
        expect(selectPlace).toHaveBeenCalledTimes(2);
        radio('Somewhere in Bern').click(); await tick();
        expect(selectPlace).toHaveBeenLastCalledWith(null);
        radio('Unnamed campsite').click(); await tick();
        expect(confirm().textContent?.trim()).toBe('End day 1 at this campsite');

        radio('Hotel Bären').click(); await tick();
        kinds = ['camp']; await tick(); await tick();
        expect(radio('Somewhere in Bern').checked).toBe(true);
        expect(confirm().textContent?.trim()).toBe('End day 1 in Bern');
    });

    it('keeps the scroll position of the day list under a night page, but not of an open day', async () => {
        const pane = document.body.appendChild(Object.assign(document.createElement('div'), { className: 'pane-scroll' }));
        Object.defineProperty(pane, 'scrollTop', { value: 0, writable: true });
        let page = $state<number | null>(null), day = $state<number | null>(null);
        mounted.push(mount(Itinerary, { target: pane, props: { ...itineraryProps(null), get nightPage() { return page; }, get expandedDay() { return day; } } }));
        const settle = async () => { await tick(); await tick(); };
        await settle();
        pane.scrollTop = 400;
        page = 1; await settle();
        expect(pane.scrollTop).toBe(0);
        page = null; await settle();
        expect(pane.scrollTop).toBe(400);
        day = 2; await settle();
        pane.scrollTop = 300;
        day = null; page = 1; await settle();
        page = null; await settle();
        expect(pane.scrollTop).toBe(0);
    });

    it('sets the kinds of place of one night', async () => {
        const props = itineraryProps(null), setKinds = vi.fn(), release = vi.fn();
        const kept = { id: 'night-2', kind: 'night' as const, night: 2, label: 'Olten', coordinate: [7.6, 47.3] as [number, number] };
        props.trip = { ...props.trip, nightStay: { 2: ['camp'] } };
        props.days = props.days.map(day => day.number === 2 ? { ...day, pinned: kept } : day);
        props.nights = nightChoices({ choosing: 2, kinds: () => ['camp'], window: () => [0, 50], found: () => ({ state: 'ready', stops: [] }), setKinds, release });
        let page = $state<number | null>(null);
        mounted.push(mount(Itinerary, { target: document.body, props: { ...props, get nightPage() { return page; } } }));
        await tick();

        page = 2; await tick();
        expect(document.querySelector('h2')?.textContent).toBe('Night 2');
        expect(document.querySelector('h2 + p')?.textContent).toBe('Where day 2 ends and day 3 starts');
        expect(document.querySelector('[role="status"]')?.textContent).toBe('No campsite between km 0 and 50.');
        button('Places to sleep on night 2: Campsites').click(); await tick();
        const huts = [...document.querySelectorAll('label')].find(row => row.textContent === 'Huts')!.querySelector('input')!;
        huts.click();
        expect(setKinds).toHaveBeenCalledWith(2, ['camp', 'hut']);
        button('Same as the trip').click();
        expect(setKinds).toHaveBeenLastCalledWith(2, null);
        button('Let the planner choose').click();
        expect(release).toHaveBeenCalledWith(2);
    });

    it('names a camping night by its campsite and keeps every suggested campsite at once', async () => {
        const props = itineraryProps(null), keepCampsites = vi.fn();
        props.nights = nightChoices({ kinds: () => ['camp'], around: () => proposal(town('Laufen', 20), ['camp']), campsites: 2, keepCampsites });
        mounted.push(mount(Itinerary, { target: document.body, props }));
        await tick();

        const night = document.querySelector('.night')!;
        expect([night.querySelector('strong')?.textContent, night.querySelector('small')?.textContent]).toEqual(['Laufen camping', 'Suggested.Laufen · 0.2 km off the route']);
        button('Keep both').click();
        expect(keepCampsites).toHaveBeenCalledOnce();
    });

    it('names a day by the towns of its nights, and a set night by its place', async () => {
        const props = itineraryProps(null);
        props.days = props.days.map(day => day.number === 1 ? { ...day, pinned: { id: 'night-1', kind: 'night', night: 1, coordinate: [8, 47], label: 'Fuxxbau', placeKind: 'alpine_hut', locality: 'Laufen' } } : day);
        props.nights = nightChoices({ around: night => night === 2 ? proposal(town('Zwingen', 20), allStay) : undefined });
        mounted.push(mount(Itinerary, { target: document.body, props }));
        await tick();

        const labels = [...document.querySelectorAll('[data-day]')].map(day => day.getAttribute('aria-label'));
        expect(labels[0]).toMatch(/^Open day 1: .+ to Laufen$/);
        expect(labels[1]).toBe('Open day 2: Laufen to Zwingen');
        const [set, suggested] = document.querySelectorAll('.night');
        expect([set.querySelector('strong')?.textContent, set.querySelector('small')?.textContent]).toEqual(['Fuxxbau', 'alpine hut · Laufen']);
        expect(suggested.querySelector('strong')?.textContent).toBe('Sleep near Zwingen');
        expect(button('Change night 1')).toBeDefined();
    });

    it('steps between the days from an open day, and shows the finish', async () => {
        const props = itineraryProps(3);
        mounted.push(mount(Itinerary, { target: document.body, props }));
        await tick();

        expect(document.querySelector('select')?.selectedOptions[0].textContent).toBe('Day 3');
        expect(document.querySelectorAll('h2')).toHaveLength(1);
        expect(button('Next riding day').disabled).toBe(true);
        button('Previous riding day').click();
        expect(props.onToggle).toHaveBeenCalledWith(2);
        const finish = [...document.querySelectorAll('button')].find(button => button.textContent?.includes('Thun') && button.textContent?.includes('Finish'))!;
        finish.click();
        expect(props.onShowEnd).toHaveBeenCalledWith(props.days[2]);
        button('All days').click();
        expect(props.onOverview).toHaveBeenCalledOnce();
    });

    it('measures a focused profile from the day start and can show the whole route', async () => {
        mounted.push(mount(Profile, {
            target: document.body,
            props: {
                total: 144, days: planView(testTrip(), testLine(testTrip())).days,
                activeNight: 2, focus: { from: 1 / 3, to: 2 / 3, label: 'Day 3' },
                window: { from: 0, to: 1 }, onToggle: vi.fn(), onNight: vi.fn(), onDayEndDrag: vi.fn(), onHover: vi.fn(),
            },
        }));
        await tick();

        const axis = () => [...document.querySelectorAll('.ticks span')].map(tick => tick.textContent);
        expect(axis()).toEqual(['0 km', '12 km', '24 km', '36 km', '48 km']);
        button('Whole route').click();
        await tick();
        expect(axis()).toEqual(['0 km', '36 km', '72 km', '108 km', '144 km']);
    });

    it('writes miles and feet for a rider on miles, and a missing ascent as a dash', async () => {
        settings.units = 'mi';
        onTestFinished(() => { settings.units = 'km'; });
        const props = itineraryProps(null);
        mounted.push(mount(RouteStats, { target: document.body, props: { distance: 48, ascent: 1000, descent: null, hours: 2 } }));
        mounted.push(mount(Itinerary, { target: document.body, props: { ...props, line: { ...props.line, unknownElevationKm: 1 } } }));
        mounted.push(mount(Profile, {
            target: document.body,
            props: {
                total: 144, days: planView(testTrip(), testLine(testTrip())).days,
                activeNight: 2, window: { from: 0, to: 1 }, onToggle: vi.fn(), onNight: vi.fn(), onDayEndDrag: vi.fn(), onHover: vi.fn(),
            },
        }));
        await tick();

        expect([...document.querySelectorAll('.stats dd')].map(figure => figure.textContent).slice(0, 3)).toEqual(['29.8mi', '3281ft', '—ft']);
        expect(document.querySelector('[data-day="1"] .figures')?.textContent).toContain('—ft');
        expect([...document.querySelectorAll('.ticks span')].map(tick => tick.textContent)).toEqual(['0 mi', '22 mi', '45 mi', '67 mi', '89 mi']);
        expect([...document.querySelectorAll('.height')].map(label => label.textContent)).toEqual(['100 ft', '0 ft']);
    });
});

describe('route shape', () => {
    it('names the route as planned a loop when it ends where it starts, and one way beside there and back', async () => {
        let trip = $state.raw<Trip>(closeLoop({ ...testTrip(), mode: 'route' }));
        const noop = () => {};
        mounted.push(mount(TripRow, { target: document.body, props: { get trip() { return trip; }, line: undefined, onChange: noop, onDays: noop, onLimit: noop,
            onShape: (next: Trip) => { trip = next; } } }));
        await tick();
        const shapes = () => [...document.querySelectorAll('[role="radio"]')].map(radio => [radio.getAttribute('aria-checked'), radio.textContent?.trim(), radio.getAttribute('title')]);
        expect(shapes()).toEqual([['true', 'Loop', null], ['false', '', 'There and back']]);
        // The arrow keys choose, as in each radio group.
        document.querySelector('[role="radio"]')!.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true })); await tick();
        expect(shapes()).toEqual([['false', '', 'One way'], ['true', 'There and back', null]]);
        expect(document.activeElement).toBe(document.querySelectorAll('[role="radio"]')[1]);
    });
});

describe('trip pace', () => {
    it('offers the pace, and a pick changes the riding time of the days', async () => {
        vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new Error('offline')));
        const props = itineraryProps(null);
        let trip = $state.raw<Trip>(props.trip);
        const onChange = vi.fn((change: Partial<Trip>) => { trip = { ...trip, ...change }; });
        mounted.push(mount(RoutePreferences, { target: document.body, props: { get trip() { return trip; }, onChange } }));
        mounted.push(mount(Itinerary, { target: document.body, props: { ...props, get trip() { return trip; },
            get days() { return planView(trip, props.line).days; } } }));
        await tick();
        const firstDay = () => document.querySelector('[data-day="1"]')!.textContent;
        const moderate = firstDay();
        document.querySelector<HTMLButtonElement>('button[aria-expanded="false"][title$="Moderate pace"]')!.click();
        await tick();
        button('Pace').click();
        await tick();
        [...document.querySelectorAll<HTMLButtonElement>('[role="option"]')].find(option => option.textContent?.includes('Fast'))!.click();
        await tick();
        expect(onChange).toHaveBeenCalledExactlyOnceWith({ pace: 'fast' }, 'Pace changed');
        expect(firstDay()).not.toBe(moderate);
        const fast = firstDay();
        onChange({ bike: 'hiking' });
        await tick();
        expect(button('Pace').textContent).toContain('Fast');
        onChange({ bike: 'road' });
        await tick();
        expect(firstDay()).not.toBe(fast);
    });
});

describe('profile part', () => {
    it('selects a part with Shift and an arrow key, shows its numbers with Clear, keeps its stretch while the map window changes and at another range, and clears at Escape and at Clear', async () => {
        const line: RoutingLine = {
            profile: 'touring', coordinates: [[8, 46], [8, 46.5], [8, 47]], elevation: [100, 600, 100], elapsed: [0, 1, 2], edges: {},
            stops: [], seconds: 2, unknownSurfaceKm: 0, pushingKm: 0, unroutedKm: 0, unknownElevationKm: 0, gapLegs: [],
        };
        let view = $state({ from: 0, to: .5 });
        let part = $state<[number, number] | null>(null);
        mounted.push(mount(Profile, { target: document.body, props: {
            lineData: line, total: 100, days: [], activeNight: 0, get window() { return view; },
            get part() { return part; }, onPart: next => { part = next; }, onToggle: vi.fn(), onNight: vi.fn(), onDayEndDrag: vi.fn(), onHover: vi.fn(),
        } }));
        await tick();
        const slider = document.querySelector<HTMLElement>('[role="slider"][aria-label="Profile"]')!;
        const key = async (shiftKey: boolean) => { slider.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', shiftKey, bubbles: true })); await tick(); };
        const select = async () => { for (let i = 0; i < 10; i++) await key(true); };
        const ticks = () => [...document.querySelectorAll('.ticks span')].map(tick => tick.textContent);

        slider.focus();
        for (let i = 0; i < 10; i++) await key(false);
        await select();
        expect(part![0]).toBeCloseTo(.05);
        expect(part![1]).toBeCloseTo(.1);
        expect(document.querySelector('.title .part')?.textContent?.replace(/\s/g, ' ')).toBe('km 5.0 to km 10.0 · 5.0 km · 50 m upClear');
        expect(document.querySelector('.distance')).toBeNull();
        const held = ticks();
        view = { from: .5, to: 1 }; await tick();
        expect(ticks()).toEqual(held);

        slider.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true })); await tick();
        expect(part).toBeNull();
        expect(ticks()).not.toEqual(held);
        await select();
        button('Clear').click(); await tick();
        expect(part).toBeNull();
        expect(document.activeElement).toBe(slider);
        await select();
        button('Whole route').click(); await tick();
        expect(part).not.toBeNull();
        expect(document.querySelector('.title .part')).not.toBeNull();
    });

    it('counts the part of an open day from its start, and shows the share of it in the day when the day end moves or another day opens', async () => {
        const line: RoutingLine = {
            profile: 'touring', coordinates: [[8, 46], [8, 46.5], [8, 47]], elevation: [100, 600, 100], elapsed: [0, 1, 2], edges: {},
            stops: [], seconds: 2, unknownSurfaceKm: 0, pushingKm: 0, unroutedKm: 0, unknownElevationKm: 0, gapLegs: [],
        };
        let part = $state<[number, number] | null>(null);
        let focus = $state({ from: .4, to: .6, label: 'Day 2' });
        mounted.push(mount(Profile, { target: document.body, props: {
            lineData: line, total: 100, days: [], activeNight: 0, get focus() { return focus; },
            get part() { return part; }, onPart: next => { part = next; }, onToggle: vi.fn(), onNight: vi.fn(), onDayEndDrag: vi.fn(), onHover: vi.fn(),
        } }));
        await tick();
        part = [.45, .5]; await tick();
        expect(document.querySelector('.title .part')?.textContent?.replace(/\s/g, ' ')).toBe('km 5.0 to km 10.0 · 5.0 km · 50 m upClear');
        expect(document.querySelector('.ticks span')?.textContent).toBe('0 km');

        focus = { from: .4, to: .55, label: 'Day 2' }; await tick();
        expect(part).toEqual([.45, .5]);
        expect(document.querySelectorAll('.pale')).toHaveLength(2);
        focus = { from: .4, to: .47, label: 'Day 2' }; await tick();
        expect(part).toEqual([.45, .5]);
        expect(document.querySelectorAll('.pale')).toHaveLength(1);
        focus = { from: .6, to: .8, label: 'Day 3' }; await tick();
        expect(part).toEqual([.45, .5]);
        expect(document.querySelector('.title .part')).toBeNull();
        expect(document.querySelectorAll('.pale')).toHaveLength(0);
    });
});

describe('profile access', () => {
    it('keeps pushing boundaries on one surface visible and keyboard-inspectable with grade colors off', async () => {
        const line: RoutingLine = {
            profile: 'road', coordinates: [[8,48], [8.001,48], [8.002,48]],
            elevation: [100, 90, 110], elapsed: [0, 20, 80], edges: { surfaces: ['Paved', 'Paved'], pushing: [false, true] },
            stops: [], seconds: 80, unknownSurfaceKm: 0, pushingKm: .075, unroutedKm: 0, unknownElevationKm: 0, gapLegs: [],
        };
        const onHover = vi.fn();
        mounted.push(mount(Profile, { target: document.body, props: {
            lineData: line, total: .15, days: [], activeNight: 0,
            onToggle: vi.fn(), onNight: vi.fn(), onDayEndDrag: vi.fn(), onHover,
        } }));
        await tick();
        expect(document.querySelector('.push-track span')).not.toBeNull();
        expect(document.querySelector('.hint')?.textContent).toContain('75 m pushing');
        const slider = document.querySelector<HTMLElement>('[role="slider"][aria-label^="Surface"]')!;
        slider.focus(); await tick();
        expect(slider.getAttribute('aria-valuetext')).toContain('Riding');
        slider.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true })); await tick();
        expect(slider.getAttribute('aria-valuetext')).toContain('Push bike');
        expect(document.querySelector('.chip')?.textContent).toContain('Push bike');
        document.querySelector<HTMLInputElement>('.grade-toggle input')!.click(); await tick();
        expect(document.querySelector('.chip')?.textContent).toContain('Push bike');
        expect(document.querySelector('.push-track span')).not.toBeNull();
        expect(onHover).toHaveBeenLastCalledWith(expect.closeTo(.75));
    });

    it('gives the surface shares of the shown stretch, and the focus on a legend entry lights its surface', async () => {
        const line: RoutingLine = {
            profile: 'gravel', coordinates: [[8,48], [8.001,48], [8.002,48], [8.003,48]],
            elevation: [100, 90, 110, 100], elapsed: [0, 20, 40, 60], edges: { surfaces: ['Paved', 'Gravel', 'Gravel'], pushing: [false, false, false] },
            stops: [], seconds: 60, unknownSurfaceKm: 0, pushingKm: 0, unroutedKm: 0, unknownElevationKm: 0, gapLegs: [],
        };
        mounted.push(mount(Profile, { target: document.body, props: {
            lineData: line, total: .225, days: [], activeNight: 0, window: { from: 0, to: .5 },
            onToggle: vi.fn(), onNight: vi.fn(), onDayEndDrag: vi.fn(), onHover: vi.fn(),
        } }));
        await tick();
        const entries = () => [...document.querySelectorAll<HTMLElement>('.surface li')];
        const legend = () => entries().map(entry => entry.textContent!.replace(/\s+/g, ' ').trim());
        expect(legend()).toEqual(['Paved 67%', 'Gravel 33%']);
        const slider = document.querySelector<HTMLElement>('[role="slider"][aria-label^="Surface"]')!;
        slider.focus(); await tick();
        expect(slider.getAttribute('aria-valuetext')).toContain('Paved · 67% of the shown stretch');
        button('Whole route').click(); await tick();
        expect(legend()).toEqual(['Gravel 67%', 'Paved 33%']);

        const dimmed = () => [...document.querySelectorAll<HTMLElement>('.track span')].map(run => run.classList.contains('dim'));
        entries()[0].focus(); await tick();
        expect(dimmed()).toEqual([true, false]);
        entries()[0].blur(); await tick();
        expect(dimmed()).toEqual([false, false]);
    });
});

describe('planner recovery', () => {
    it('returns keyboard focus to the place that opened the callout', async () => {
        const opener = document.createElement('button');
        opener.textContent = 'Inspect place';
        document.body.append(opener);
        opener.focus();
        const onClose = vi.fn();
        const trip = testTrip();
        mounted.push(mount(MapCallout, {
            target: document.body,
            props: {
                kind: 'place', trip, days: planView(trip, undefined).days, night: 1,
                place: { ...places[0], website: 'camp.example', phone: '+49 (123) 45-67', description: 'Small tents only.\n<script>Ask at reception.</script>' },
                coordinate: places[0].coordinate, legMode: 'routed',
                onClose, onAddHere: vi.fn(), onLegMode: vi.fn(), onInsert: vi.fn(), onPick: vi.fn(),
                onChooseNight: vi.fn(), onStay: vi.fn(), onAddVisit: vi.fn(), onRename: vi.fn(), onKind: vi.fn(), onRemove: vi.fn(),
            },
        }));
        await tick();
        await new Promise(requestAnimationFrame);
        expect(document.querySelector('.place-note')?.textContent).toBe('Small tents only.\n<script>Ask at reception.</script>');
        expect(document.querySelector('.place-note script')).toBeNull();
        expect(document.querySelector('a[href="https://camp.example/"]')?.textContent).toBe('Website');
        expect(document.querySelector('a[href="tel:+491234567"]')?.textContent).toBe('+49 (123) 45-67');
        expect(document.activeElement?.getAttribute('role')).toBe('dialog');
        button('Close').click();
        expect(onClose).toHaveBeenCalledOnce();
        expect(document.activeElement).toBe(opener);
    });
});

describe('save', () => {
    beforeEach(() => {
        vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new Error('offline')));
        HTMLElement.prototype.showPopover ??= () => {};
        HTMLDialogElement.prototype.showModal ??= function (this: HTMLDialogElement) { this.open = true; };
    });
    const bar = (props: { stored: boolean; changed: boolean; canSave: boolean; more?: { label: string; disabled?: boolean; run: () => void }[] }) =>
        mounted.push(mount(TripBar, { target: document.body, props: { ...saveProps, ...props, trip: testTrip(), name: 'Lake loop', ready: true,
            canUndo: false, canRedo: false, onUndo: vi.fn(), onRedo: vi.fn() } }));
    const saveButton = () => document.querySelector<HTMLButtonElement>('.trip-bar .save > button')!;

    it.each([
        ['a new empty route', { stored: false, changed: false, canSave: false }, 'Save', true],
        ['a new route with points', { stored: false, changed: false, canSave: true }, 'Save', false],
        ['a stored route with no changes', { stored: true, changed: false, canSave: false }, 'Saved', true],
        ['a stored route with changes', { stored: true, changed: true, canSave: true }, 'Save', false],
    ])('shows the Save button of %s', async (_, props, label, disabled) => {
        bar(props);
        await tick();
        expect([saveButton().textContent?.trim(), saveButton().getAttribute('aria-disabled')]).toEqual([label, String(disabled)]);
        // Only a stored route has a title that renames it.
        expect(!!document.querySelector('.trip-bar h1 button')).toBe(props.stored);
    });

    it('lists "Save a copy…", "Discard changes" and "Compare routes…" in the "More" menu', async () => {
        const compare = vi.fn();
        bar({ stored: true, changed: false, canSave: false, more: [{ label: 'Save a copy…', run: vi.fn() },
            { label: 'Discard changes', disabled: true, run: vi.fn() }, { label: 'Compare routes…', run: compare }] });
        await tick();
        button('More ▾').click();
        await tick();
        expect([...document.querySelectorAll('menu button')].map(item => [item.textContent?.trim(), (item as HTMLButtonElement).disabled]))
            .toEqual([['Save a copy…', false], ['Discard changes', true], ['Compare routes…', false]]);
        button('Compare routes…').click();
        await tick();
        expect([compare.mock.calls.length, document.querySelector('menu')]).toEqual([1, null]);
    });

    /** The name field and the buttons of the save window for `trip`. */
    async function saveWindow(mode: SaveMode, trip: Trip, stored = false) {
        const onAnswer = vi.fn();
        mounted.push(mount(SaveWindow, { target: document.body, props: { mode, name: stored ? 'Lake loop' : '', stored, trip, onAnswer } }));
        await tick();
        const input = document.querySelector<HTMLInputElement>('dialog input');
        return { onAnswer, input, title: document.querySelector('dialog h2')!.textContent,
            buttons: [...document.querySelectorAll<HTMLButtonElement>('dialog .window-buttons button')].map(b => [b.textContent, b.disabled]) };
    }
    // A finish with no label keeps its coordinates as its label.
    const named = (start: string, finish?: string): Trip => ({ ...testTrip(), points: testTrip().points.map(p => p.kind === 'start' ? { ...p, label: start }
        : p.kind === 'finish' ? { ...p, label: finish ?? coordinateName(p.coordinate) } : p) });

    it('offers the title of the route as its name, and an empty field while the start or the finish has no place name', async () => {
        const first = await saveWindow('save', named('Basel', 'Bern'));
        expect([first.title, first.input?.value, first.buttons]).toEqual(['Save route', 'Basel → Bern', [['Cancel', false], ['Save', false]]]);
        document.body.replaceChildren();
        const unnamed = await saveWindow('save', named('Basel'));
        expect([unnamed.input?.value, unnamed.input?.placeholder, unnamed.buttons[1]]).toEqual(['', 'Name of the route', ['Save', true]]);
        unnamed.input!.value = 'Jura';
        unnamed.input!.dispatchEvent(new Event('input', { bubbles: true }));
        await tick();
        expect(document.querySelector<HTMLButtonElement>('dialog button[type="submit"]')!.disabled).toBe(false);
        document.body.replaceChildren();
        const question = await saveWindow('leave', named('Basel', 'Bern'), true);
        expect([question.title, question.input, question.buttons.map(([label]) => label)]).toEqual(['Save changes to ‘Lake loop’?', null, ['Cancel', 'Discard', 'Save']]);
        document.body.replaceChildren();
        const copy = await saveWindow('copy', named('Basel', 'Bern'), true);
        expect([copy.title, copy.input?.value]).toEqual(['Save a copy', 'Lake loop (copy)']);
    });
});
