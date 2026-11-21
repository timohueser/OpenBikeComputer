import { afterEach, describe, expect, it, vi } from 'vitest';
import { FakeCaches } from '../../../../test-support/fake-caches';
import type { Models } from '../models/model-file';
import { encodeModels, pack } from '../models/pack';
import { ModelFiles } from './model-files';

const triangle = { position: [0, 0, 0, 1, 0, 0, 0, 1, 1], normal: [0, 0, 1, 0, 0, 1, 0, 0, 1], color: [0.2, 0.4, 0.6, 0.2, 0.4, 0.6, 0.2, 0.4, 0.6], index: [0, 1, 2] };

describe('the model files', () => {
    afterEach(() => (vi.unstubAllGlobals(), vi.restoreAllMocks()));

    it('come from the device cache once a load saved them; a refused file is not fetched again, a server error is; the old tree cache goes', async () => {
        const caches = new FakeCaches();
        vi.stubGlobal('caches', caches);
        // The tree cache of earlier builds goes when the store opens.
        await caches.open('trailview-trees/release/version');
        vi.spyOn(console, 'warn').mockImplementation(() => {});
        const bytes = await encodeModels([pack([triangle])]);
        const fetch = vi.fn(async (url: string) => (url === 'good' ? new Response(bytes) : new Response(null, { status: url === 'gone' ? 404 : 503 })));
        vi.stubGlobal('fetch', fetch);
        const first = new ModelFiles({} as Models, () => {}, new AbortController().signal);
        const [model] = (await first.load('good'))!;
        expect([await first.load('gone'), await first.load('gone'), await first.load('busy'), await first.load('busy'), fetch.mock.calls.length]).toEqual([undefined, undefined, undefined, undefined, 4]);
        expect(first.done('good', 'gone', 'busy')).toBe(true);
        first.save();
        await new Promise(resolve => setTimeout(resolve, 20));
        const again = await new ModelFiles({} as Models, () => {}, new AbortController().signal).load('good');
        expect([fetch.mock.calls.length, (await caches.keys()).some(name => name.startsWith('trailview-trees/'))]).toEqual([4, false]);
        // toStrictEqual compares the classes of the typed arrays too.
        expect(again![0]).toStrictEqual(model);
    });
});
