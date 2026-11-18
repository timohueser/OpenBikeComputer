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

function open(dataLayers: DataLayer[] = []) {
    shown = mount(MapSettings, { target: document.body, props: {
        dataLayers, hillshade: true, contours: true, hidden: [], highlighted: [], mapOverlays: { network: 'cycling', access: true },
    } });
    click('Map settings');
}

function click(label: string) {
    [...document.querySelectorAll('button')].find(button => (button.getAttribute('aria-label') ?? button.textContent?.trim()) === label)!.click();
    flushSync();
}

const row = (label: string) => document.querySelector(`[role="group"][aria-label="${label}"]`);
const lit = (label: string) => [...row(label)?.querySelectorAll('[aria-pressed="true"]') ?? []].map(tile => tile.textContent?.trim());

it('lights one route network at a time, and a second click removes the network and its legend', () => {
    open();
    expect(lit('Routes, one at a time')).toEqual(['Cycling']);
    click('Hiking');
    expect(lit('Routes, one at a time')).toEqual(['Hiking']);
    expect(document.querySelector('[aria-label="Route network colors"]')).not.toBeNull();
    click('Hiking');
    expect(lit('Routes, one at a time')).toEqual([]);
    expect(document.querySelector('[aria-label="Route network colors"]')).toBeNull();
});

it('shows a conditions tile for each data layer of the region, and no row without one', () => {
    open([{ id: 'snow', label: 'Snow' } as DataLayer]);
    expect([...row('Conditions, one at a time')!.querySelectorAll('button')].map(tile => tile.textContent?.trim())).toEqual(['Snow']);
    unmount(shown!);
    open();
    expect(row('Conditions, one at a time')).toBeNull();
    expect(row('Map')).not.toBeNull();
});
