import { describe, expect, it, vi } from 'vitest';
import { articleVariant, onlinePhoto, type PlaceContent } from './place-content';

const photo: NonNullable<PlaceContent['photo']> = {
    source_url: 'https://commons.wikimedia.org/?oldid=42', revision: '42', license_url: 'https://creativecommons.org/licenses/by/4.0/',
    online_url: 'https://upload.wikimedia.org/wikipedia/commons/thumb/a/ab/Image.jpg/500px-Image.jpg', credit: ['Image', 'Artist', 'CC BY 4.0', 'Commons'],
    file_identity: { filename: 'Image.jpg', page_id: 7 }, page_revision: 42, file_revision: { timestamp: '2026-01-01T00:00:00Z', sha1: 'abc' },
};
const page = () => ({ pageid: 7, lastrevid: 42, title: 'File:Image.jpg', imageinfo: [{ timestamp: photo.file_revision!.timestamp, sha1: 'abc', thumburl: photo.online_url }] });

describe('shared place content', () => {
    it('selects English and then the prepared default, never an unrelated translation', () => {
        const content: PlaceContent = { default_language: 'de', variants: ['de', 'en'].map(language => ({ language, text_pages: ['Text'], attribution: photo })) };
        expect(articleVariant(content)?.language).toBe('en');
        expect(articleVariant(content, 'fr')?.language).toBe('de');
    });
    it('shows only a current thumbnail whose file and credit identities match', async () => {
        const fetcher = vi.fn().mockResolvedValue({ ok: true, json: async () => ({ query: { pages: [page()] } }) });
        vi.stubGlobal('fetch', fetcher);
        try {
            expect(await onlinePhoto(photo, new AbortController().signal)).toBe(photo.online_url);
            expect(fetcher).toHaveBeenCalledTimes(1);
            expect(String(fetcher.mock.calls[0][0])).toContain('iiurlwidth=500');
            for (const changed of [{ ...page(), lastrevid: 43 }, { ...page(), pageid: 8 },
                { ...page(), imageinfo: [{ ...page().imageinfo[0], sha1: 'different' }] },
                { ...page(), imageinfo: [{ ...page().imageinfo[0], timestamp: 'later' }] },
                { ...page(), imageinfo: [{ ...page().imageinfo[0], thumburl: 'https://upload.wikimedia.org/original.jpg' }] }]) {
                fetcher.mockResolvedValue({ ok: true, json: async () => ({ query: { pages: [changed] } }) });
                expect(await onlinePhoto(photo, new AbortController().signal)).toBeUndefined();
            }
            const count = fetcher.mock.calls.length;
            expect(await onlinePhoto({ ...photo, page_revision: null }, new AbortController().signal)).toBeUndefined();
            expect(fetcher).toHaveBeenCalledTimes(count);
        } finally { vi.unstubAllGlobals(); }
    });
});
