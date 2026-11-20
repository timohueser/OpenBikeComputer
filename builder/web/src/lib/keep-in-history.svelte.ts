// Keeps a part of a page's state in the current history entry, so that Back to the page brings it again.

import { on } from 'svelte/events';
import { parseRoute } from './routes';

const KEEP_AFTER_MS = 300;

/**
 * Writes the snapshot of `read()` into `history.state[key]` after each change, and with `address` the address that it
 * gives into the entry too. Call it while a component starts. It gives a function that writes at once: call it before
 * code changes the entry; a click on a link calls it by itself.
 */
export function keepInHistory(key: string, read: () => unknown, address?: () => string | undefined): () => void {
    // WebKit refuses many history writes in a short time, so it writes once after the last change.
    let keep: (() => void) | undefined;
    $effect(() => {
        const value = $state.snapshot(read()), url = address?.(), route = parseRoute(location.hash).route;
        keep = () => {
            keep = undefined;
            // An address of another page since then, such as Back or the settings window, is no entry of this page.
            if (parseRoute(location.hash).route === route) history.replaceState({ ...history.state, [key]: value }, '', url);
        };
        // After a link wrote the state, `keep` is undefined: the timer must not write into the next entry.
        const timer = setTimeout(() => keep?.(), KEEP_AFTER_MS);
        return () => clearTimeout(timer);
    });
    $effect(() => on(window, 'click', event => { if ((event.target as Element).closest?.('a[href]')) keep?.(); }, { capture: true }));
    return () => keep?.();
}
