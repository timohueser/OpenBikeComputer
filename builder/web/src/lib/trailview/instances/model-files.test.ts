import { afterEach, describe, expect, it, vi } from 'vitest';
import { FakeCaches } from '../../../../test-support/fake-caches';
import type { Models } from '../models/model-file';
import { encodeModels, pack } from '../models/pack';
import { ModelFiles } from './model-files';

const triangle = { position: [0, 0, 0, 1, 0, 0, 0, 1, 1], normal: [0, 0, 1, 0, 0, 1, 0, 0, 1], color: [0.2, 0.4, 0.6, 0.2, 0.4, 0.6, 0.2, 0.4, 0.6], index: [0, 1, 2] };

describe('the model files', () => {
    afterEach(() => (vi.unstubAllGlobals(), vi.restoreAllMocks()));

    it('come from the device cache once a load saved them, and a file that failed is not fetched again', async () => {
        vi.stubGlobal('caches', new FakeCaches());
        vi.spyOn(console, 'warn').mockImplementation(() => {});
        const bytes = await encodeModels([pack([triangle])]);
        const fetch = vi.fn(async (url: string) => (url === 'good' ? new Response(bytes) : new Response(null, { status: 404 })));
        vi.stubGlobal('fetch', fetch);
        const first = new ModelFiles({} as Models, new AbortController().signal);
        const [model] = (await first.load('good'))!;
        expect([await first.load('gone'), await first.load('gone'), fetch.mock.calls.length]).toEqual([undefined, undefined, 2]);
        first.save();
        await new Promise(resolve => setTimeout(resolve, 20));
        const again = await new ModelFiles({} as Models, new AbortController().signal).load('good');
        expect(fetch.mock.calls.length).toBe(2);
        // toStrictEqual compares the classes of the typed arrays too.
        expect(again![0]).toStrictEqual(model);
    });
});
