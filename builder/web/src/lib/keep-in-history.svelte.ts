// Keeps a part of a page's state in the current history entry, so that Back to the page brings it again.

import { on } from 'svelte/events';

const KEEP_AFTER_MS = 300;

/**
 * Writes the snapshot of `read()` into `history.state[key]` after each change. Call it while a component starts. It gives
 * a function that writes at once: call it before code changes the entry; a click on a link calls it by itself.
 */
export function keepInHistory(key: string, read: () => unknown): () => void {
    // WebKit refuses many history writes in a short time, so it writes once after the last change.
    let keep: (() => void) | undefined;
    $effect(() => {
        const value = $state.snapshot(read());
        keep = () => { keep = undefined; history.replaceState({ ...history.state, [key]: value }, ''); };
        // After a link wrote the state, `keep` is undefined: the timer must not write into the next entry.
        const timer = setTimeout(() => keep?.(), KEEP_AFTER_MS);
        return () => clearTimeout(timer);
    });
    $effect(() => on(window, 'click', event => { if ((event.target as Element).closest?.('a[href]')) keep?.(); }, { capture: true }));
    return () => keep?.();
}
