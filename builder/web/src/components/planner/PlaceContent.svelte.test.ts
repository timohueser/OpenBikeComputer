// @vitest-environment happy-dom
import { mount, tick, unmount } from 'svelte';
import { expect, it, vi } from 'vitest';
import PlaceContent from './PlaceContent.svelte';

it('keeps the short article and source credits readable when an online photo is unavailable', async () => {
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new TypeError('Offline')));
    const component = mount(PlaceContent, { target: document.body, props: { name: 'Castle', content: {
        default_language: 'en', variants: [{ language: 'en', text_pages: ['A castle above the valley.'], attribution: {
            source_url: 'https://en.wikipedia.org/?oldid=42', revision: '42', license_url: 'https://creativecommons.org/licenses/by-sa/4.0/',
        } }], photo: { source_url: 'https://commons.wikimedia.org/?oldid=7', revision: '7', license_url: 'https://creativecommons.org/licenses/by/4.0/',
            online_url: null, credit: ['Castle', 'Artist', 'CC BY 4.0', 'Commons'] },
    } } });
    try {
        await tick(); await Promise.resolve(); await tick();
        expect(document.body.textContent).toContain('A castle above the valley.');
        expect(document.body.textContent).toContain('Photo unavailable. Photos need an internet connection.');
        expect(document.body.textContent).toContain('Artist');
        expect(document.querySelector('a')?.href).toBe('https://en.wikipedia.org/?oldid=42');
        expect([...document.querySelectorAll('a')].find(a => a.textContent === 'Photo source')?.href).toBe('https://commons.wikimedia.org/?oldid=7');
        expect(document.querySelector('img')).toBeNull();
        expect(fetch).not.toHaveBeenCalled();
    } finally { await unmount(component); vi.unstubAllGlobals(); document.body.replaceChildren(); }
});
