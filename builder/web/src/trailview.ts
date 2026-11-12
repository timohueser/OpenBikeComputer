import { plannerConfig, type PlannerConfig } from './lib/planner/config';
import { activeRelease, pageRelease } from './lib/planner/release';
import { TrailView } from './lib/trailview/engine';
import { Gpu } from './lib/trailview/gpu/device';
import { hud } from './lib/trailview/hud';
import { scaleOf } from './lib/trailview/resolve/targets';
import { still, type StillJob } from './lib/trailview/still';
import { coverView } from './lib/trailview/still-view';
import { defaultView, formatView, parseView } from './lib/trailview/view';

/** The address follows the view at most this often. */
const ADDRESS_MS = 200;

function notice(text: string): void {
    const message = document.createElement('p');
    message.className = 'notice';
    message.setAttribute('role', 'alert');
    message.textContent = text;
    document.body.replaceChildren(message);
}

/** The device and the release; undefined after the notice that the browser has no WebGPU. */
async function open(): Promise<{ gpu: Gpu; release: PlannerConfig } | undefined> {
    const preview: string | undefined = import.meta.env.VITE_PLANNER_CONFIG;
    const config = (async () => plannerConfig(preview ? JSON.parse(preview) : await activeRelease()))();
    config.catch(() => {});
    const gpu = await Gpu.request();
    if (!gpu) return void notice('TrailView needs WebGPU. Open this page in a current version of Chrome, Edge or Safari.');
    // `start` reports a failed compile.
    TrailView.prepare(gpu).catch(() => {});
    const release = await config;
    pageRelease(release.id);
    return { gpu, release };
}

/** With `still` in the address, the page starts no engine: a headless script renders stills through `window.trailviewStill`. */
async function stills(): Promise<void> {
    const opened = await open();
    if (!opened) return;
    const { gpu, release } = opened;
    Object.assign(window, {
        trailviewStill: {
            still: (job: StillJob, options?: Parameters<typeof still>[3]) => still(gpu, release, job, options),
            coverView: (line: [number, number][], when: Parameters<typeof coverView>[2]) => coverView(line, release, when),
        },
    });
}

async function main(): Promise<void> {
    const opened = await open();
    if (!opened) return;
    const { gpu, release } = opened;
    const view = parseView(location.search, defaultView(release.bounds));
    const engine = await TrailView.start(document.querySelector('canvas')!, gpu, release, view, scaleOf(location.search));

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

(new URLSearchParams(location.search).has('still') ? stills() : main()).catch((error: unknown) => notice(error instanceof Error ? error.message : String(error)));
