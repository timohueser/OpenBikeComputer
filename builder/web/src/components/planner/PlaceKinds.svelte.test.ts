// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, expect, it, vi } from 'vitest';
import PlaceKinds from './PlaceKinds.svelte';

let shown: ReturnType<typeof mount> | undefined;
afterEach(() => {
    if (shown) unmount(shown);
    document.body.replaceChildren();
});

const button = (label: string) => document.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`);
const pressed = (label: string) => button(label)?.getAttribute('aria-pressed');
const checkbox = (name: string) => [...document.querySelectorAll('label')].find(label => label.textContent?.trim() === name)!.querySelector('input')!;

it('hides all places with one click, and a kind or a highlight shows them again with one highlighted kind', () => {
    vi.stubGlobal('localStorage', { getItem: () => null, setItem: () => {} });
    shown = mount(PlaceKinds, { target: document.body });
    button('Hide places')!.click();
    flushSync();
    expect(pressed('Show places')).toBe('false');
    button('Kinds of places')!.click();
    flushSync();
    expect(button('Kinds of places')!.getAttribute('aria-expanded')).toBe('true');

    button('Highlight Water')!.click();
    flushSync();
    expect(pressed('Hide places')).toBe('true');
    expect(pressed('Highlight Water')).toBe('true');
    button('Highlight Lodging')!.click();
    flushSync();
    expect([pressed('Highlight Water'), pressed('Highlight Lodging')]).toEqual(['false', 'true']);
    expect(checkbox('Lodging').checked).toBe(true);

    checkbox('Lodging').click();
    flushSync();
    expect(checkbox('Lodging').checked).toBe(false);
    expect(pressed('Highlight Lodging')).toBe('false');
});
