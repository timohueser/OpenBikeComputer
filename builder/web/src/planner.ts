import { mount } from 'svelte';
import './styles/tokens.css';
import { startCoverJob } from './lib/covers/job';
import { lineKey, sentKey } from './lib/library/summary';
import { Library } from './lib/library/store';
import { setLook } from './lib/theme.svelte';

const target = document.getElementById('app')!;
const search = new URLSearchParams(location.search), cover = search.get('cover');
if (cover !== null) {
    // The page of the browser test of a cover; the test fills the library through `window.coverTest`.
    const library = new Library();
    Object.assign(window, { coverTest: { library, lineKey, sentKey, startCoverJob: () => startCoverJob(library), setLook } });
    void import('./lib/covers/CoverPage.svelte').then(({ default: CoverPage }) => mount(CoverPage, { target, props: { library, id: cover } }));
} else {
    // The dev server and the build of the browser tests can put a pretend device on the cable; another build drops this branch.
    const fake = (import.meta.env.DEV || import.meta.env.VITE_FAKE_DEVICE) && search.has('fake-device')
        ? import('../test-support/device-sync/fake-device').then(({ fakeDevice }) => fakeDevice()) : Promise.resolve();
    // A dynamic import, so the test page of a cover has none of the shell's base rules.
    fake.then(() => import('./Shell.svelte')).then(({ default: Shell }) => mount(Shell, { target }), (error: unknown) => {
        const message = document.createElement('p');
        message.setAttribute('role', 'alert');
        message.style.cssText = 'margin: 32px 16px; font-family: var(--sans);';
        message.textContent = error instanceof Error ? error.message : String(error);
        target.replaceChildren(message);
    });
}
