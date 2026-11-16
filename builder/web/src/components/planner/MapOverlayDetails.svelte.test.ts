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

it('shows relations of one name, number and network as one route, with the plan and the website of any of them', async () => {
    const route = { kind: 'cycling', network: 'ncn', rank: 3, name: 'Seen-Route', ref: '9' };
    const onplan = vi.fn();
    shown = mount(MapOverlayDetails, { target: document.body, props: {
        selection: { coordinate: [8.3, 47], way: 1, kind: 'cycling', routes: [
            { ...route, id: 1 }, { ...route, id: 2, website: 'https://www.example.org/seen' },
            { kind: 'cycling', rank: 0, name: 'Alpenpanorama-Route', id: 3 }] },
        onclose: () => {}, onplan,
        canPlan: async (id: number) => id === 2 ? 'plan' as const : 'missing' as const,
    } });
    await tick(); await tick();
    const rows = [...document.querySelectorAll('li')];
    expect(rows.map(row => row.querySelector('strong')!.textContent)).toEqual(['Seen-Route', 'Alpenpanorama-Route']);
    expect(rows[0].querySelector('small')!.textContent!.replace(/\s+/g, ' ').trim()).toBe('National · example.org');
    expect(rows[1].querySelector('button')).toBeNull();
    rows[0].querySelector('button')!.click();
    expect(onplan).toHaveBeenCalledWith(2);
});
