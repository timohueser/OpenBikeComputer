// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, expect, it } from 'vitest';
import MapSettings from './MapSettings.svelte';
import type { DataLayer } from '../../lib/planner/layers/data-layer';

let shown: ReturnType<typeof mount> | undefined;
afterEach(() => {
    if (shown) unmount(shown);
    shown = undefined;
    document.body.replaceChildren();
});

function open(dataLayers: DataLayer[] = [], ways = false) {
    shown = mount(MapSettings, { target: document.body, props: {
        dataLayers, ways, contours: true, slopeAngle: false, mapOverlays: { network: 'cycling', access: true },
    } });
    click('Map settings');
}

function click(label: string) {
    [...document.querySelectorAll('button')].find(button => (button.getAttribute('aria-label') ?? button.textContent?.trim()) === label)!.click();
    flushSync();
}

const row = (label: string) => document.querySelector(`[role="group"][aria-label="${label}"]`);
const tiles = (label: string) => [...row(label)!.querySelectorAll('button')].map(tile => tile.textContent?.trim());
const lit = (label: string) => [...row(label)?.querySelectorAll('[aria-pressed="true"]') ?? []].map(tile => tile.textContent?.trim());

it('lights one route network at a time, and shows no legend: the legends are on the map', () => {
    open();
    expect(lit('Routes, one at a time')).toEqual(['Cycling']);
    click('Hiking');
    expect(lit('Routes, one at a time')).toEqual(['Hiking']);
    expect(document.querySelector('ul')).toBeNull();
    click('Hiking');
    expect(lit('Routes, one at a time')).toEqual([]);
});

it('shows a conditions tile for each data layer of the region, and no row without one', () => {
    open([{ id: 'snow', label: 'Snow' } as DataLayer]);
    expect(tiles('Conditions, one at a time')).toEqual(['Snow']);
    unmount(shown!);
    open();
    expect(row('Conditions, one at a time')).toBeNull();
    expect(row('Map')).not.toBeNull();
});

it('shows the row of the roads only for a release with the ways, and lights one of its layers at a time', () => {
    open();
    expect(row('Roads, one at a time')).toBeNull();
    unmount(shown!);
    open([], true);
    expect(tiles('Roads, one at a time')).toEqual(['Steepness', 'Trail grade']);
    expect(lit('Roads, one at a time')).toEqual([]);
    click('Steepness');
    click('Trail grade');
    expect(lit('Roads, one at a time')).toEqual(['Trail grade']);
    click('Trail grade');
    expect(lit('Roads, one at a time')).toEqual([]);
});

it('switches the slope angle in the map row, which has no switch for the relief', () => {
    open();
    expect(tiles('Map')).toEqual(['Contours', 'Slope angle', 'Closures']);
    expect(lit('Map')).toEqual(['Contours', 'Closures']);
    click('Slope angle');
    expect(lit('Map')).toEqual(['Contours', 'Slope angle', 'Closures']);
});
