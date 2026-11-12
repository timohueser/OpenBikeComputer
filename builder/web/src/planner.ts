import { mount } from 'svelte';
import './styles/tokens.css';
import { initCore } from './lib/core/bridge';
import { lineKey, startCoverJob } from './lib/covers/job';
import { Library } from './lib/library/store';

const target = document.getElementById('app')!;
// One library for the page: a listener of `onChange` hears only the writes of its own object.
const library = new Library();
const cover = new URLSearchParams(location.search).get('cover');
if (cover !== null) {
    // The page of the browser test of a cover; the test fills the library through `window.coverTest`.
    Object.assign(window, { coverTest: { library, lineKey } });
    void import('./lib/covers/CoverPage.svelte').then(({ default: CoverPage }) => mount(CoverPage, { target, props: { library, id: cover } }));
} else {
    // The planner reads the live catalogue as it loads. A catalogue that it cannot use leaves a short message.
    initCore().then(() => import('./components/planner/PlannerApp.svelte')).then(({ default: PlannerApp }) => {
        mount(PlannerApp, { target, props: { library } });
        startCoverJob(library);
    }, (error: unknown) => {
        const message = document.createElement('p');
        message.setAttribute('role', 'alert');
        message.style.cssText = 'margin: 32px 16px; font-family: var(--sans);';
        message.textContent = error instanceof Error ? error.message : String(error);
        target.replaceChildren(message);
    });
}
