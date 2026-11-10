import { plannerConfig } from './lib/planner/config';
import { activeRelease, pageRelease } from './lib/planner/release';
import { TrailView } from './lib/trailview/engine';
import { Gpu } from './lib/trailview/gpu/device';
import { hud } from './lib/trailview/hud';
import { preload } from './lib/trailview/plan/bridge';
import { WORKERS } from './lib/trailview/terrain/model';
import { scaleOf } from './lib/trailview/resolve/targets';
import { defaultView, finestOf, formatView, groundPlanOf, parseView, poolOf } from './lib/trailview/view';

/** The address follows the view at most this often. */
const ADDRESS_MS = 200;
const NO_WEBGPU = 'TrailView needs WebGPU. Open this page in a current version of Chrome, Edge or Safari.';

function notice(text: string): void {
    const message = document.createElement('p');
    message.className = 'notice';
    message.setAttribute('role', 'alert');
    message.textContent = text;
    document.body.replaceChildren(message);
}

async function main(): Promise<void> {
    if (!navigator.gpu) return notice(NO_WEBGPU);
    const preview: string | undefined = import.meta.env.VITE_PLANNER_CONFIG;
    const config = (async () => plannerConfig(preview ? JSON.parse(preview) : await activeRelease()))();
    config.catch(() => {});
    // The release, the device, the plan's bridge and its workers load side by side; `start` reports a failed load. The
    // pipelines compile at the attach: a compile from the device on would hold the start of the ground's workers, whose
    // WebGL waits for the same GPU process.
    const plan = groundPlanOf(location.search);
    if (plan) preload(WORKERS);
    const gpu = Gpu.request().then(gpu => {
        if (!gpu) throw new Error(NO_WEBGPU);
        return gpu;
    });
    gpu.catch(() => {});
    const release = await config;
    pageRelease(release.id);
    const view = parseView(location.search, defaultView(release.bounds));
    const engine = await TrailView.start(document.querySelector('canvas')!, gpu, release, view, scaleOf(location.search), plan, finestOf(location.search), poolOf(location.search));

    const address = () => `${location.origin}${location.pathname}${formatView(engine.view, location.search)}`;
    let writing: ReturnType<typeof setTimeout> | undefined;
    engine.onView = () => {
        writing ??= setTimeout(() => {
            writing = undefined;
            history.replaceState(null, '', address());
        }, ADDRESS_MS);
    };
    engine.onView(view);
    // Ctrl+C copies the view as an address, and Ctrl+V shows a pasted address or query string.
    addEventListener('copy', event => {
        if (getSelection()?.toString()) return;
        event.clipboardData?.setData('text/plain', address());
        event.preventDefault();
    });
    addEventListener('paste', event => {
        const text = event.clipboardData?.getData('text/plain').trim();
        if (!text) return;
        engine.view = parseView(text.includes('?') ? text.slice(text.indexOf('?')) : text, engine.view);
        event.preventDefault();
    });
    const credits = document.querySelector<HTMLElement>('#credits')!;
    credits.textContent = [release.terrain_attribution, ...engine.attribution].filter(Boolean).join(' · ');
    credits.hidden = !credits.textContent;
    hud(document.querySelector<HTMLElement>('#hud')!, engine);
    Object.assign(window, { trailView: engine });
}

main().catch((error: unknown) => notice(error instanceof Error ? error.message : String(error)));
