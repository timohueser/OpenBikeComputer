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

it('gives a new rider all places with the default kinds and no highlight', async () => {
    const { placeChoice } = await load();
    expect(placeChoice).toEqual({ shown: true, kinds: defaultKinds, highlight: null });
});

it('keeps the valid fields of a stored value, and drops an unknown kind and a highlight of a kind that is off', async () => {
    expect((await load('not json')).placeChoice).toEqual({ shown: true, kinds: defaultKinds, highlight: null });
    vi.resetModules();
    expect((await load('[1]')).placeChoice).toEqual({ shown: true, kinds: defaultKinds, highlight: null });
    vi.resetModules();
    const { placeChoice } = await load(JSON.stringify({ shown: false, kinds: ['peak', 'casino', 'water'], highlight: 'food' }));
    expect(placeChoice).toEqual({ shown: false, kinds: ['water', 'peak'], highlight: null });
});

it('hides all places, switches a kind, keeps one highlight, and stores each change', async () => {
    const { placeChoice, togglePlaces, toggleKind, toggleHighlight } = await load(JSON.stringify({ shown: true, kinds: ['water'], highlight: null }));
    togglePlaces();
    expect(placeChoice).toEqual({ shown: false, kinds: ['water'], highlight: null });
    toggleHighlight('hotel');
    expect(placeChoice).toEqual({ shown: true, kinds: ['water', 'hotel'], highlight: 'hotel' });
    toggleHighlight('water');
    expect(placeChoice).toMatchObject({ kinds: ['water', 'hotel'], highlight: 'water' });
    togglePlaces();
    toggleKind('water');
    expect(placeChoice).toEqual({ shown: true, kinds: ['hotel'], highlight: null });
    expect(JSON.parse(stored.get('obc-planner-places')!)).toEqual(placeChoice);
});
