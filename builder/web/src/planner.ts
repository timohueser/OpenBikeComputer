import { mount } from 'svelte';
import './styles/tokens.css';
import { lineKey, startCoverJob } from './lib/covers/job';
import { Library } from './lib/library/store';
import { setLook } from './lib/theme.svelte';

const target = document.getElementById('app')!;
const cover = new URLSearchParams(location.search).get('cover');
if (cover !== null) {
    // The page of the browser test of a cover; the test fills the library through `window.coverTest`.
    const library = new Library();
    Object.assign(window, { coverTest: { library, lineKey, startCoverJob: () => startCoverJob(library), setLook } });
    void import('./lib/covers/CoverPage.svelte').then(({ default: CoverPage }) => mount(CoverPage, { target, props: { library, id: cover } }));
} else {
    // A dynamic import, so the test page of a cover has none of the shell's base rules.
    import('./Shell.svelte').then(({ default: Shell }) => mount(Shell, { target }), (error: unknown) => {
        const message = document.createElement('p');
        message.setAttribute('role', 'alert');
        message.style.cssText = 'margin: 32px 16px; font-family: var(--sans);';
        message.textContent = error instanceof Error ? error.message : String(error);
        target.replaceChildren(message);
    });
}
