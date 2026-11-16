// @vitest-environment happy-dom
import { mount, tick, unmount } from 'svelte';
import { afterEach, expect, it, vi } from 'vitest';
import MapOverlayDetails from './MapOverlayDetails.svelte';

let shown: ReturnType<typeof mount> | undefined;
afterEach(() => {
    if (shown) unmount(shown);
    shown = undefined;
    document.body.replaceChildren();
});

it('shows relations of one name, number and network as one route: the one that Plan plans, with the website of any of them', async () => {
    const route = { kind: 'cycling', network: 'ncn', rank: 3, name: 'Seen-Route', ref: '9' };
    const stage = { kind: 'cycling', network: 'rcn', rank: 2, name: 'Rhein-Route', ref: '2' };
    const onplan = vi.fn();
    shown = mount(MapOverlayDetails, { target: document.body, props: {
        selection: { coordinate: [8.3, 47], way: 1, kind: 'cycling', routes: [
            { ...route, id: 1 }, { ...route, id: 2, website: 'https://www.example.org/seen' },
            // A whole route that is too long and its stage of the same name stay two rows.
            { ...stage, id: 3 }, { ...stage, id: 4 },
            // The tiles leave out empty properties: routes with no name and no number are not one route.
            { kind: 'cycling', rank: 0, id: 5 }, { kind: 'cycling', rank: 0, id: 6 }] },
        onclose: () => {}, onplan,
        canPlan: async (id: number) => id === 2 || id === 4 ? 'plan' as const : id === 3 ? 'too-long' as const : 'missing' as const,
    } });
    await tick(); await tick();
    const rows = [...document.querySelectorAll('li')];
    const text = (row: Element, selector: string) => [...row.querySelectorAll(selector)].map(node => node.textContent!.replace(/\s+/g, ' ').trim());
    expect(rows.map(row => text(row, 'strong')[0])).toEqual(['Seen-Route', 'Rhein-Route', 'Rhein-Route', 'Unnamed route', 'Unnamed route']);
    expect(text(rows[0], 'small')).toEqual(['National · example.org']);
    expect(text(rows[2], 'small')[1]).toContain('Too long for one plan');
    expect(rows.map(row => !!row.querySelector('button'))).toEqual([true, true, false, false, false]);
    rows[0].querySelector('button')!.click();
    rows[1].querySelector('button')!.click();
    expect(onplan.mock.calls).toEqual([[2], [4]]);
});
