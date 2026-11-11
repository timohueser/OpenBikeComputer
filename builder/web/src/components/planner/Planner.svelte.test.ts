// @vitest-environment happy-dom

import { mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { addRestDay, allStay, planView, type Place, type Stay } from '../../lib/planner/editor';
import { testLine, testTrip } from '../../../test-support/planner/trip';
import type { NightChoices } from '../../lib/planner/night-stops.svelte';
import { proposal, type Stop } from '../../lib/planner/stops';
import Itinerary from './Itinerary.svelte';
import Profile from './PlannerProfile.svelte';
import MapCallout from './MapCallout.svelte';
import VersionsMenu from './VersionsMenu.svelte';
import Select from './PlannerSelect.svelte';
import RouteList from './RouteList.svelte';
import type { RoutingLine } from '../../lib/planner/routing';
import { newVersion } from '../../lib/planner/versions';

const places: Place[] = [{id:'test-camp',kind:'place',label:'Test camp',category:'camp',description:'',coordinate:[7.5,47.5]}];

const mounted: ReturnType<typeof mount>[] = [];

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
    return { choosing: null, stopId: null, shownId: null, kinds: () => allStay, found: () => undefined, window: () => null, around: () => undefined, neighbours: () => [undefined, undefined],
        campsites: 0, keepCampsites: vi.fn(),
        date: () => undefined, elevationAt: () => null, choose: vi.fn(), open: vi.fn(), step: vi.fn(), endHere: vi.fn(), stay: vi.fn(),
        selectPlace: vi.fn(), pick: vi.fn(), retry: vi.fn(), setKinds: vi.fn(), release: vi.fn(), ...choices };
}

const town = (name: string, km: number): Stop => ({ id: name, name, km, from: km, to: km, food: [], shop: [],
    sleep: [{ place: { id: `${name}-camp`, kind: 'place', category: 'camp', placeKind: 'campsite', label: `${name} camping`, description: '', coordinate: [7.6, 47.2], locality: name }, km, off: .2 }] });

function itineraryProps(expandedDay: number | null) {
    const trip = addRestDay(testTrip(), 1), line = testLine(trip);
    return {
        trip, line, expandedDay, nightPage: null as number | null, itinerary: planView(trip, line).itinerary, days: planView(trip, line).days, theme: 'light' as const,
        nights: nightChoices(), conflicts: [], selectedId: null, revealId: null,
        onToggle: vi.fn(), onOverview: vi.fn(), onInspect: vi.fn(), onShowEnd: vi.fn(),
        onShowConflict: vi.fn(), onAddRest: vi.fn(), onTransfer: vi.fn(), onRemoveRest: vi.fn(), onNameRest: vi.fn(),
    };
}

describe('planner day views', () => {
    it('keeps every riding day in the overview and opens the right day after a rest', async () => {
        const props = itineraryProps(null);
        mounted.push(mount(Itinerary, { target: document.body, props }));
        await tick();

        expect(document.querySelectorAll('[data-day]')).toHaveLength(3);
        expect(document.body.textContent).toContain('Day 2 · Rest');
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
        expect(towns[1].getAttribute('aria-label')).toMatch(/^Bern, 1 campsite\. Day 1 \d+h \d\dm, day 3 \d+h \d\dm\. Even days$/);
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

    it('adds a rest day or a train after a night, and sets the kinds of place of one night', async () => {
        const props = itineraryProps(null), setKinds = vi.fn(), release = vi.fn();
        const kept = { id: 'night-2', kind: 'night' as const, night: 2, label: 'Olten', coordinate: [7.6, 47.3] as [number, number] };
        props.trip = { ...props.trip, nightStay: { 2: ['camp'] } };
        props.days = props.days.map(day => day.number === 2 ? { ...day, pinned: kept } : day);
        props.nights = nightChoices({ choosing: 2, kinds: () => ['camp'], window: () => [0, 50], found: () => ({ state: 'ready', stops: [] }), setKinds, release });
        let page = $state<number | null>(null);
        mounted.push(mount(Itinerary, { target: document.body, props: { ...props, get nightPage() { return page; } } }));
        await tick();

        // Night 1 has a rest day after it already.
        expect(document.querySelector('[aria-label="Rest day after night 1"]')).toBeNull();
        button('Train or bus after night 1').click();
        expect(props.onTransfer).toHaveBeenCalledWith(1);
        button('Rest day after night 3').click();
        expect(props.onAddRest).toHaveBeenCalledWith(2);

        page = 2; await tick();
        expect(document.querySelector('h2')?.textContent).toBe('Night 3');
        expect(document.querySelector('h2 + p')?.textContent).toBe('Where day 3 ends and day 4 starts');
        expect(document.querySelector('[role="status"]')?.textContent).toBe('No campsite between km 0 and 50.');
        button('Places to sleep on night 3: Campsites').click(); await tick();
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

    it('keeps calendar labels separate from riding-day navigation and the finish', async () => {
        const props = itineraryProps(3);
        mounted.push(mount(Itinerary, { target: document.body, props }));
        await tick();

        expect(document.querySelector('select')?.selectedOptions[0].textContent).toBe('Day 4');
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
                total: 144, days: planView(testTrip(), testLine(testTrip())).days, dayLabels: { 1: 1, 2: 3, 3: 4 },
                activeNight: 2, band: null, focus: { from: 1 / 3, to: 2 / 3, label: 'Day 3' },
                window: { from: 0, to: 1 }, onToggle: vi.fn(), onNight: vi.fn(), onDayEndDrag: vi.fn(), onHover: vi.fn(),
            },
        }));
        await tick();

        const axis = () => [...document.querySelectorAll('.axis span')].map(tick => tick.textContent);
        expect(axis()).toEqual(['0 km', '12 km', '24 km', '36 km', '48 km']);
        button('Whole route').click();
        await tick();
        expect(axis()).toEqual(['0 km', '36 km', '72 km', '108 km', '144 km']);
    });
});

describe('trip timeline', () => {
    it('puts a rest column after its night', async () => {
        const trip = { ...testTrip(), restAfter: [2] };
        mounted.push(mount(Profile, { target: document.body, props: {
            total: 89, days: planView(trip, testLine(trip)).days, dayLabels: { 1: 1, 2: 2, 3: 4 }, singleRoute: false,
            rests: [{ after: 2, name: 'Spa day', town: 'Olten' }], activeNight: 0, band: null, onToggle: vi.fn(), onNight: vi.fn(), onDayEndDrag: vi.fn(), onHover: vi.fn(),
        } }));
        await tick();
        button('Whole route').click(); await tick();

        // profile-axis.test.ts measures where the column sits; here it is the one column, of the night after day 2.
        expect([...document.querySelectorAll('.rest')].map(rest => rest.textContent)).toEqual(['Spa dayOlten']);
    });
});

describe('profile access', () => {
    it('keeps pushing boundaries on one surface visible and keyboard-inspectable with grade colors off', async () => {
        const line: RoutingLine = {
            profile: 'road', coordinates: [[8,48], [8.001,48], [8.002,48]],
            elevation: [100, 90, 110], elapsed: [0, 20, 80], edges: { surfaces: ['Paved', 'Paved'], pushing: [false, true] },
            stops: [], seconds: 80, unknownSurfaceKm: 0, pushingKm: .075, unroutedKm: 0, unknownElevationKm: 0,
        };
        const onHover = vi.fn();
        mounted.push(mount(Profile, { target: document.body, props: {
            lineData: line, total: .15, days: [], dayLabels: {}, activeNight: 0, band: null,
            onToggle: vi.fn(), onNight: vi.fn(), onDayEndDrag: vi.fn(), onHover,
        } }));
        await tick();
        expect(document.querySelector('.push-track span')).not.toBeNull();
        expect(document.querySelector('.hint')?.textContent).toContain('75 m pushing');
        const slider = document.querySelector<HTMLElement>('[role="slider"]')!;
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
                kind: 'place', trip, days: planView(trip, undefined).days, dayLabels: { 1: 1, 2: 2, 3: 3 }, night: 1,
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

    it('names the open version, opens another, and compares only two or more', async () => {
        const onOpen = vi.fn(), onCompare = vi.fn(), onChange = vi.fn().mockResolvedValue(undefined);
        const coast = newVersion(testTrip(), undefined, 'Coast'), hills = newVersion(testTrip(), undefined, 'Hills');
        let versions = $state.raw([coast]), openId = $state<string | undefined>(coast.id);
        mounted.push(mount(VersionsMenu, { target: document.body, props: { trip: testTrip(), get versions() { return versions; }, get openId() { return openId; },
            draftSavedAt: null, draftError: '', onOpen, onSaveNew: vi.fn(), onChange, onCompare } }));
        await tick();
        button('Version Coast').click();
        await tick();
        expect(button('Compare versions').disabled).toBe(true);
        versions = [hills, coast];
        await tick();
        expect(document.querySelector('[aria-current="true"]')?.textContent).toContain('Coast');
        button('Compare versions').click();
        expect(onCompare).toHaveBeenCalledOnce();
        button('Version Coast').click();
        await tick();
        document.querySelector<HTMLButtonElement>('[aria-current="false"]')!.click();
        expect(onOpen).toHaveBeenCalledWith(hills.id, 'Hills');
        button('Version Coast').click();
        await tick();
        button('Rename this version…').click();
        await tick();
        const input = document.querySelector('input')!;
        input.value = 'Coast road';
        input.dispatchEvent(new Event('input', { bubbles: true }));
        input.form!.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }));
        expect(onChange).toHaveBeenCalledWith([hills, { ...coast, name: 'Coast road' }]);
        openId = undefined;
        await tick();
        expect(button('Save version')).toBeDefined();
    });

    it('keeps a failed version save editable and lets the rider retry', async () => {
        const onSaveNew = vi.fn().mockRejectedValueOnce(new Error('Storage full')).mockResolvedValue(undefined);
        mounted.push(mount(VersionsMenu, {
            target: document.body,
            props: { trip: testTrip(), versions: [], draftSavedAt: null, draftError: '', onOpen: vi.fn(), onSaveNew, onChange: vi.fn(), onCompare: vi.fn() },
        }));
        await tick();
        button('Save version').click();
        await tick();
        document.querySelector('form')!.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }));
        await tick();
        expect(document.querySelector('[role="alert"]')?.textContent).toContain('Could not save');
        expect(document.querySelector('input')).not.toBeNull();
        expect(onSaveNew).toHaveBeenCalledTimes(1);

        document.querySelector('form')!.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }));
        await tick();
        expect(onSaveNew).toHaveBeenCalledTimes(2);
        expect(document.querySelector('[role="alert"]')).toBeNull();
    });
});
