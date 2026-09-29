// @vitest-environment happy-dom

import { mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { addRestDay, initialTrip, itineraryDays, tripDays, type Place } from '../../lib/planner/editor';
import Itinerary from './Itinerary.svelte';
import Profile from './PlannerProfile.svelte';
import MapCallout from './MapCallout.svelte';
import VersionsMenu from './VersionsMenu.svelte';
import Select from './PlannerSelect.svelte';
import RouteList from './RouteList.svelte';
import type { RoutingLine } from '../../lib/planner/routing';

const places: Place[] = [{id:'test-camp',kind:'place',label:'Test camp',category:'camp',description:'',progress:.25,coordinate:[7.5,47.5]}];

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
        mounted.push(mount(RouteList, { target: document.body, props: { stops, onReorder, onInspect: vi.fn() } }));
        await tick();
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
        expect(document.querySelector('[role="status"]')?.textContent).toBe('First moved to stop 3 of 3.');
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

function itineraryProps(expandedDay: number | null) {
    const trip = addRestDay(initialTrip(), 1);
    return {
        trip, expandedDay, itinerary: itineraryDays(trip), days: tripDays(trip), theme: 'light' as const,
        changing: false, candidates: [], conflicts: [], selectedId: null, revealId: null,
        onToggle: vi.fn(), onOverview: vi.fn(), onInspect: vi.fn(), onShowEnd: vi.fn(),
        onSelectPlace: vi.fn(), onPick: vi.fn(), onChangeOvernight: vi.fn(), onEditTarget: vi.fn(),
        onShowConflict: vi.fn(), onAddRest: vi.fn(), onRemoveRest: vi.fn(), onNameRest: vi.fn(),
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

    it('keeps calendar labels separate from riding-day navigation and the finish', async () => {
        const props = itineraryProps(3);
        mounted.push(mount(Itinerary, { target: document.body, props }));
        await tick();

        expect(document.querySelector('select')?.selectedOptions[0].textContent).toBe('Day 4');
        expect(document.querySelectorAll('h2')).toHaveLength(1);
        expect(button('Next riding day').disabled).toBe(true);
        button('Previous riding day').click();
        expect(props.onToggle).toHaveBeenCalledWith(2);
        const finish = [...document.querySelectorAll('button')].find(button => button.textContent?.includes('Besançon') && button.textContent?.includes('Finish'))!;
        finish.click();
        expect(props.onShowEnd).toHaveBeenCalledWith(props.days[2]);
        button('All days').click();
        expect(props.onOverview).toHaveBeenCalledOnce();
    });

    it('measures a focused profile from the day start and can show the whole route', async () => {
        mounted.push(mount(Profile, {
            target: document.body,
            props: {
                total: 144, days: tripDays(initialTrip()), dayLabels: { 1: 1, 2: 3, 3: 4 },
                activeNight: 2, band: null, focus: { from: 1 / 3, to: 2 / 3, label: 'Day 3' },
                window: { from: 0, to: 1 }, onNight: vi.fn(), onDayEndDrag: vi.fn(), onHover: vi.fn(),
            },
        }));
        await tick();

        const axis = () => [...document.querySelectorAll('.axis span')].map(tick => tick.textContent);
        expect(axis()).toEqual(['0 km', '12 km', '24 km', '36 km', '48 km']);
        expect(document.querySelector('.distance')?.textContent).toBe('48.0 km');
        button('Whole route').click();
        await tick();
        expect(axis()).toEqual(['0 km', '36 km', '72 km', '108 km', '144 km']);
        expect(document.querySelector('.distance')?.textContent).toBe('144.0 km');
    });
});

describe('profile access', () => {
    it('keeps pushing boundaries on one surface visible and keyboard-inspectable with grade colors off', async () => {
        const line: RoutingLine = {
            key: 'test', choiceId: 'test', profile: 'road', coordinates: [[8,48], [8.001,48], [8.002,48]],
            elevation: [100, 90, 110], elapsed: [0, 20, 80], surfaces: ['Paved', 'Paved'], pushing: [false, true],
            stops: [], seconds: 80, alternatives: [], alternativesReady: true, unknownSurfaceKm: 0, pushingKm: .075, unroutedKm: 0,
        };
        const onHover = vi.fn();
        mounted.push(mount(Profile, { target: document.body, props: {
            lineData: line, total: .15, days: [], dayLabels: {}, activeNight: 0, band: null,
            onNight: vi.fn(), onDayEndDrag: vi.fn(), onHover,
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
        const trip = initialTrip();
        mounted.push(mount(MapCallout, {
            target: document.body,
            props: {
                kind: 'place', trip, days: tripDays(trip), dayLabels: { 1: 1, 2: 2, 3: 3 }, night: 1,
                place: places[0], coordinate: places[0].coordinate, candidates: [], legMode: 'routed',
                onClose, onAddHere: vi.fn(), onLegMode: vi.fn(), onInsert: vi.fn(), onPick: vi.fn(),
                onSelectPlace: vi.fn(), onStay: vi.fn(), onAddVisit: vi.fn(), onRename: vi.fn(), onKind: vi.fn(), onRemove: vi.fn(),
            },
        }));
        await tick();
        await new Promise(requestAnimationFrame);
        expect(document.activeElement?.getAttribute('role')).toBe('dialog');
        button('Close').click();
        expect(onClose).toHaveBeenCalledOnce();
        expect(document.activeElement).toBe(opener);
    });

    it('keeps a failed version save editable and lets the rider retry', async () => {
        const onSaved = vi.fn();
        mounted.push(mount(VersionsMenu, {
            target: document.body,
            props: { trip: initialTrip(), draftSavedAt: null, draftError: '', onRestore: vi.fn(), onSaved },
        }));
        await tick();
        button('Save').click();
        await tick();
        vi.spyOn(localStorage, 'setItem').mockImplementationOnce(() => { throw new Error('Storage full'); });
        document.querySelector('form')!.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }));
        await tick();
        expect(document.querySelector('[role="alert"]')?.textContent).toContain('Could not save');
        expect(document.querySelector('input')).not.toBeNull();
        expect(onSaved).not.toHaveBeenCalled();

        document.querySelector('form')!.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }));
        await tick();
        expect(onSaved).toHaveBeenCalledOnce();
        expect(document.querySelector('[role="alert"]')).toBeNull();
    });
});
