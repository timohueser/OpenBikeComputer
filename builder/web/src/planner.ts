import { mount } from 'svelte';
import './styles/tokens.css';
import { lineKey } from './lib/covers/job';
import { Library } from './lib/library/store';

const target = document.getElementById('app')!;
const search = new URLSearchParams(location.search), cover = search.get('cover');
if (cover !== null) {
    // The page of the browser test of a cover; the test fills the library through `window.coverTest`.
    const library = new Library();
    Object.assign(window, { coverTest: { library, lineKey } });
    void import('./lib/covers/CoverPage.svelte').then(({ default: CoverPage }) => mount(CoverPage, { target, props: { library, id: cover } }));
} else if (search.has('add-files')) {
    // The page of the window "Add files", until a page of the product opens it. The test reads the library through
    // `window.addFilesTest`.
    const library = new Library();
    Object.assign(window, { addFilesTest: { library } });
    void Promise.all([import('./lib/core/bridge').then(({ initCore }) => initCore()), import('./pages/AddFiles.svelte'),
        import('./lib/theme.svelte'), import('./styles/shell.css')]).then(([, { default: AddFiles }, { startTheme }]) => {
        startTheme();
        mount(AddFiles, { target, props: { library, onClose: () => {} } });
    });
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
