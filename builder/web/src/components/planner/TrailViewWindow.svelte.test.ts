// @vitest-environment happy-dom
import { createRawSnippet, flushSync, mount, unmount } from 'svelte';
import { afterEach, expect, it, vi } from 'vitest';
import { defaultView } from '../../lib/trailview/view';
import TrailViewWindow from './TrailViewWindow.svelte';

vi.mock('../../lib/planner/trailview', async original => ({
    ...await original<typeof import('../../lib/planner/trailview')>(),
    loadTrailView: async () => ({ startTrailView: async () => undefined }),
}));

// happy-dom has no Web Animations.
Element.prototype.animate ??= () => ({}) as Animation;

let shown: ReturnType<typeof mount> | undefined;
afterEach(() => {
    if (shown) unmount(shown);
    shown = undefined;
    document.body.replaceChildren();
});

it('shows the card of the routes mode beside the notice, with no buttons of a plan', async () => {
    const card = createRawSnippet(() => ({ render: () => '<p class="test-card">Seen-Route</p>' }));
    shown = mount(TrailViewWindow, { target: document.body, props: {
        routes: [{ id: '1', line: [[8.3, 47], [8.31, 47.01]], colour: '#cc2a93' }], view: defaultView([8.3, 47, 8.31, 47.01]),
        card, origin: [0, 0], onClose: () => {},
    } });
    await vi.waitFor(() => {
        flushSync();
        expect(document.querySelector('[role="alert"]')).not.toBeNull();
    });
    expect(document.querySelector('.test-card')?.textContent).toBe('Seen-Route');
    expect([...document.querySelectorAll('button')].map(button => button.textContent)).not.toContain('Route');
});
