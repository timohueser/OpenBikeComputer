// @vitest-environment happy-dom
import { mount, unmount } from 'svelte';
import { afterEach, expect, it } from 'vitest';
import MapOverlayDetails from './MapOverlayDetails.svelte';

let shown: ReturnType<typeof mount> | undefined;
afterEach(() => {
    if (shown) unmount(shown);
    shown = undefined;
    document.body.replaceChildren();
});

it('shows relations of one name, number and network as one route, with the website of any of them', () => {
    const route = { kind: 'cycling', network: 'ncn', rank: 3, name: 'Seen-Route', ref: '9' };
    shown = mount(MapOverlayDetails, { target: document.body, props: {
        selection: { coordinate: [8.3, 47], way: 1, kind: 'cycling', routes: [
            { ...route, id: 1 }, { ...route, id: 2, website: 'https://www.example.org/seen' },
            // The tiles leave out empty properties: routes with no name and no number are not one route.
            { kind: 'cycling', rank: 0, id: 5 }, { kind: 'cycling', rank: 0, id: 6 }] },
        onclose: () => {},
    } });
    const rows = [...document.querySelectorAll('li')];
    const text = (row: Element, selector: string) => [...row.querySelectorAll(selector)].map(node => node.textContent!.replace(/\s+/g, ' ').trim());
    expect(rows.map(row => text(row, 'strong')[0])).toEqual(['Seen-Route', 'Unnamed route', 'Unnamed route']);
    expect(text(rows[0], 'small')).toEqual(['National · example.org']);
    expect(text(rows[1], 'small')).toEqual([]);
    expect(document.querySelector('li button')).toBeNull();
});
