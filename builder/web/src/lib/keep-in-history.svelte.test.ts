// @vitest-environment happy-dom
import { flushSync } from 'svelte';
import { afterEach, expect, it, vi } from 'vitest';
import { keepInHistory } from './keep-in-history.svelte';

afterEach(() => vi.useRealTimers());

it('writes the state and the address of its page, and nothing while another page shows, such as the settings window', () => {
    vi.useFakeTimers();
    history.replaceState(null, '', '#/explore');
    let value = $state(1);
    const stop = $effect.root(() => { keepInHistory('explore', () => value, () => `#/explore?v=${value}`); });
    flushSync();
    vi.advanceTimersByTime(300);
    expect([location.hash, history.state.explore]).toEqual(['#/explore?v=1', 1]);
    history.pushState(null, '', '#/settings');
    value = 2;
    flushSync();
    vi.advanceTimersByTime(300);
    expect([location.hash, history.state]).toEqual(['#/settings', null]);
    stop();
});
