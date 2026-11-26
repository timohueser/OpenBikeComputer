// @vitest-environment happy-dom
import { createRawSnippet, flushSync, mount, unmount } from 'svelte';
import { afterEach, expect, it } from 'vitest';
import Callout from './Callout.svelte';

let shown: ReturnType<typeof mount> | undefined;
afterEach(() => {
    if (shown) unmount(shown);
    shown = undefined;
    document.body.replaceChildren();
});

const frame = () => new Promise(resolve => requestAnimationFrame(resolve));

/** Two buttons, the first with the focus, and the popup opened from it. */
function open() {
    const [opener, other] = ['Opener', 'Other'].map(name => document.body.appendChild(Object.assign(document.createElement('button'), { textContent: name })));
    opener.focus();
    shown = mount(Callout, { target: document.body, props: { label: 'Place', onClose: () => {}, children: createRawSnippet(() => ({ render: () => '<p>Body</p>' })) } });
    flushSync();
    return { other, callout: document.querySelector<HTMLElement>('.callout')! };
}

it('takes the focus from its opener in its first frame', async () => {
    const { callout } = open();
    await frame();
    expect(document.activeElement).toBe(callout);
});

it('leaves the focus on a control that the rider chose before that frame', async () => {
    const { other } = open();
    other.focus();
    await frame();
    expect(document.activeElement).toBe(other);
});

it('gives a key to the part that names it, Enter to the amber tile, and leaves a key in a field to the field', () => {
    shown = mount(Callout, { target: document.body, props: { label: 'Point', onClose: () => {}, children: createRawSnippet(() => ({
        render: () => '<div><input><button data-key="Delete">Remove</button><button data-key="V">Visit</button><button class="go">Sleep</button></div>' })) } });
    flushSync();
    const callout = document.querySelector<HTMLElement>('.callout')!;
    const clicked: string[] = [];
    callout.querySelectorAll('button').forEach(button => button.addEventListener('click', () => clicked.push(button.textContent!)));
    const press = (target: Element, key: string) => target.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true }));
    press(callout.querySelector('input')!, 'Backspace');
    press(callout.querySelector('input')!, 'v');
    expect(clicked).toEqual([]);
    for (const key of ['Delete', 'Backspace', 'v', 'Enter']) press(callout, key);
    expect(clicked).toEqual(['Remove', 'Remove', 'Visit', 'Sleep']);
});
