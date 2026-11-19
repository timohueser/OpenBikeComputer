// @vitest-environment happy-dom
import { beforeEach, expect, it, vi } from 'vitest';
import { defaultKinds } from './poi-kinds';

const stored = new Map<string, string>();
beforeEach(() => {
    stored.clear();
    vi.stubGlobal('localStorage', { getItem: (key: string) => stored.get(key) ?? null, setItem: (key: string, value: string) => stored.set(key, value) });
    vi.resetModules();
});

/** The module as a page load finds it, with `value` in the storage. */
async function load(value?: string) {
    if (value !== undefined) stored.set('obc-planner-places', value);
    return import('./place-choice.svelte');
}

it('gives a new rider the default kinds and no highlight', async () => {
    const { placeChoice } = await load();
    expect(placeChoice).toEqual({ kinds: defaultKinds, highlight: null });
});

it('keeps the valid fields of a stored value, and drops an unknown field, an unknown kind and a highlight of a kind that is off', async () => {
    expect((await load('not json')).placeChoice).toEqual({ kinds: defaultKinds, highlight: null });
    vi.resetModules();
    expect((await load('[1]')).placeChoice).toEqual({ kinds: defaultKinds, highlight: null });
    vi.resetModules();
    const { placeChoice } = await load(JSON.stringify({ shown: false, kinds: ['peak', 'casino', 'water'], highlight: 'food' }));
    expect(placeChoice).toEqual({ kinds: ['water', 'peak'], highlight: null });
});

it('switches a kind, keeps one highlight on a kind that is on, and stores each change', async () => {
    const { placeChoice, toggleKind, toggleHighlight } = await load(JSON.stringify({ kinds: ['water'], highlight: null }));
    toggleHighlight('hotel');
    expect(placeChoice).toEqual({ kinds: ['water', 'hotel'], highlight: 'hotel' });
    toggleHighlight('water');
    expect(placeChoice).toEqual({ kinds: ['water', 'hotel'], highlight: 'water' });
    toggleKind('water');
    expect(placeChoice).toEqual({ kinds: ['hotel'], highlight: null });
    expect(JSON.parse(stored.get('obc-planner-places')!)).toEqual(placeChoice);
});

it('takes the change of another tab, so that a change here keeps it', async () => {
    const { placeChoice, toggleKind } = await load(JSON.stringify({ kinds: ['water', 'toilets'], highlight: null }));
    stored.set('obc-planner-places', JSON.stringify({ kinds: ['water', 'toilets'], highlight: 'water' }));
    window.dispatchEvent(new StorageEvent('storage', { key: 'obc-planner-places' }));
    expect(placeChoice.highlight).toBe('water');
    toggleKind('toilets');
    expect(JSON.parse(stored.get('obc-planner-places')!)).toEqual({ kinds: ['water'], highlight: 'water' });
});
