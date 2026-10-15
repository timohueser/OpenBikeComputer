import { describe, expect, it, vi } from 'vitest';
import { StopFinder, stretchKey } from './night-stops.svelte';
import { searchPlaces, type SearchContext } from './search/types';

vi.mock('./search/types', () => ({ searchPlaces: vi.fn() }));
const search = vi.mocked(searchPlaces);
const context = {} as SearchContext;
const settled = () => new Promise(resolve => setTimeout(resolve));

describe('StopFinder', () => {
    it('searches a failed stretch again on retry, and stops the search of a stretch no longer wanted', async () => {
        search.mockRejectedValue(new Error('offline'));
        const finder = new StopFinder(), first: [number, number] = [10, 30], second: [number, number] = [40, 60];
        finder.want('route', context, [first]);
        await settled();
        expect(finder.found[stretchKey('route', first)].state).toBe('error');

        search.mockReset().mockResolvedValue({ results: [] } as never);
        finder.retry(stretchKey('route', first));
        await settled();
        expect(finder.found[stretchKey('route', first)]).toEqual({ state: 'ready', stops: [] });

        search.mockReset().mockReturnValue(new Promise(() => {}));
        finder.want('route', context, [second]);
        const signal = search.mock.calls[0][3];
        finder.want('other', context, [first]);
        expect([signal.aborted, finder.found[stretchKey('route', second)], finder.found[stretchKey('route', first)]?.state]).toEqual([true, undefined, 'ready']);
    });
});
