// @vitest-environment happy-dom
import { flushSync } from 'svelte';
import { afterEach, expect, it, vi } from 'vitest';
import { keepInHistory } from './keep-in-history.svelte';

afterEach(() => vi.useRealTimers());

it('writes the state and the address of its page; while another page shows, as the settings window, the write waits for the next call', () => {
    vi.useFakeTimers();
    history.replaceState(null, '', '#/explore');
    let value = $state(1);
    let keep = () => {};
    const stop = $effect.root(() => { keep = keepInHistory('explore', () => value, () => `#/explore?v=${value}`); });
    flushSync();
    vi.advanceTimersByTime(300);
    expect([location.hash, history.state.explore]).toEqual(['#/explore?v=1', 1]);
    history.pushState(null, '', '#/settings');
    value = 2;
    flushSync();
    vi.advanceTimersByTime(300);
    expect([location.hash, history.state]).toEqual(['#/settings', null]);
    // Back on the page, the next call writes what the page skipped.
    history.replaceState(null, '', '#/explore?v=1');
    keep();
    expect([location.hash, history.state.explore]).toEqual(['#/explore?v=2', 2]);
    stop();
});
