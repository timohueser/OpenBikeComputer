// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, expect, it, vi } from 'vitest';
import type { Pointer } from '../../lib/trailview/frame/controls';
import { defaultView } from '../../lib/trailview/view';
import TrailViewWindow from './TrailViewWindow.svelte';

const engine = {
    view: defaultView([8.3, 47, 8.31, 47.01]), attribution: [] as string[],
    showRoutes: vi.fn(), boldRoute: vi.fn(), routeAt: vi.fn(), destroy: vi.fn(),
};
let pointer: Pointer = {};
vi.mock('../../lib/planner/trailview', async original => ({
    ...await original<typeof import('../../lib/planner/trailview')>(),
    loadTrailView: async () => ({
        startTrailView: async (...args: unknown[]) => {
            pointer = args[5] as Pointer;
            return engine;
        },
        viewSettled: async () => true,
    }),
}));

// The small map needs WebGL, which happy-dom lacks.
vi.mock('./TrailViewMiniMap.svelte', () => ({ default: () => {} }));

// happy-dom has no Web Animations.
Element.prototype.animate ??= () => ({}) as Animation;

let shown: ReturnType<typeof mount> | undefined;
afterEach(() => {
    if (shown) unmount(shown);
    shown = undefined;
    document.body.replaceChildren();
});

it('selects the route under a click, and makes the selected route bold when the pointer leaves the view', async () => {
    const line: [number, number][] = [[8.3, 47], [8.31, 47.01]];
    const onselect = vi.fn();
    shown = mount(TrailViewWindow, { target: document.body, props: {
        routes: [{ id: 'a', line, colour: '#cc2a93' }, { id: 'b', line, colour: '#1f5fd6' }], view: engine.view,
        selected: 'a', onselect, origin: [0, 0], onClose: () => {},
    } });
    await vi.waitFor(() => expect(engine.showRoutes).toHaveBeenCalledOnce());

    engine.routeAt.mockReturnValue(2);
    pointer.onclick!(10, 10);
    expect(onselect).toHaveBeenCalledWith('b');

    pointer.onhover!(10, 10);
    expect(engine.boldRoute).toHaveBeenLastCalledWith(2);
    document.querySelector('canvas')!.dispatchEvent(new Event('pointerleave'));
    flushSync();
    expect(engine.boldRoute).toHaveBeenLastCalledWith(1);
});
