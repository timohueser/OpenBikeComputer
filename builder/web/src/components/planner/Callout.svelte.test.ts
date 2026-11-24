// @vitest-environment happy-dom
import { createRawSnippet, flushSync, mount, unmount } from 'svelte';
import { afterEach, expect, it, vi } from 'vitest';
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

it('removes with Delete or Backspace, and leaves a key in a field to the field', () => {
    const onDelete = vi.fn();
    shown = mount(Callout, { target: document.body, props: { label: 'Point', onClose: () => {}, onDelete, children: createRawSnippet(() => ({ render: () => '<input>' })) } });
    flushSync();
    const callout = document.querySelector<HTMLElement>('.callout')!;
    const press = (target: Element, key: string) => target.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true }));
    press(callout.querySelector('input')!, 'Backspace');
    expect(onDelete).not.toHaveBeenCalled();
    press(callout, 'Delete');
    press(callout, 'Backspace');
    expect(onDelete).toHaveBeenCalledTimes(2);
});
