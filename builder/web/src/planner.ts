import { mount } from 'svelte';
import './styles/tokens.css';
import { initCore } from './lib/core/bridge';

const target = document.getElementById('app')!;
// The planner reads the live catalogue as it loads. A catalogue that it cannot use leaves a short message.
initCore().then(() => import('./components/planner/PlannerApp.svelte')).then(({ default: PlannerApp }) => mount(PlannerApp, { target }), (error: unknown) => {
    const message = document.createElement('p');
    message.setAttribute('role', 'alert');
    message.style.cssText = 'margin: 32px 16px; font-family: var(--sans);';
    message.textContent = error instanceof Error ? error.message : String(error);
    target.replaceChildren(message);
});
