// @vitest-environment happy-dom
import { createRawSnippet, flushSync, mount, unmount } from 'svelte';
import { afterEach, expect, it, vi } from 'vitest';
import Switch from './Switch.svelte';
import Window from './Window.svelte';

let shown: ReturnType<typeof mount> | undefined;
afterEach(() => {
    if (shown) unmount(shown);
    shown = undefined;
    document.body.replaceChildren();
});

it('a switch turns over on a click and gives its state to the binding', () => {
    const props = $state({ checked: false, label: 'Show rides' });
    shown = mount(Switch, { target: document.body, props });
    const toggle = document.querySelector<HTMLButtonElement>('[role="switch"]')!;
    expect(toggle.textContent).toContain('Show rides');
    expect(toggle.getAttribute('aria-checked')).toBe('false');
    toggle.click();
    flushSync();
    expect(toggle.getAttribute('aria-checked')).toBe('true');
    expect(props.checked).toBe(true);
});

it('a window opens as a modal dialog and calls onClose when its button closes it', async () => {
    const onClose = vi.fn();
    const children = createRawSnippet(() => ({ render: () => '<p>Body</p>' }));
    shown = mount(Window, { target: document.body, props: { title: 'Export', onClose, children } });
    flushSync();
    const dialog = document.querySelector('dialog')!;
    expect(dialog.open).toBe(true);
    expect(document.getElementById(dialog.getAttribute('aria-labelledby')!)?.textContent).toBe('Export');
    expect(dialog.textContent).toContain('Body');
    document.querySelector<HTMLButtonElement>('button[aria-label="Close"]')!.click();
    await vi.waitFor(() => expect(onClose).toHaveBeenCalledOnce());
    expect(dialog.open).toBe(false);
});
