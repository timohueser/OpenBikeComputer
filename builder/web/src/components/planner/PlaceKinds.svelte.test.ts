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

it('opens the kinds, and a highlight and a checkbox change the pressed states with one highlighted kind', () => {
    vi.stubGlobal('localStorage', { getItem: () => null, setItem: () => {} });
    shown = mount(PlaceKinds, { target: document.body });
    button('Kinds of places')!.click();
    flushSync();
    expect(button('Kinds of places')!.getAttribute('aria-expanded')).toBe('true');

    button('Highlight Water')!.click();
    flushSync();
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
