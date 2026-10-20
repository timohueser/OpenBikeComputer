import type { TrailView } from './engine';
import type { Labs } from './labs';
import { DEBUG_VIEWS, type DebugView } from './view';

const MB = 2 ** 20;
const REFRESH_MS = 250;

/** The lab HUD: the GPU time per pass, the CPU time, the MB per pool, the requests, the view and the debug view. H hides it. */
export function hud(element: HTMLElement, engine: TrailView, labs: Labs): void {
    const text = document.createElement('pre');
    const debug = document.createElement('select');
    debug.setAttribute('aria-label', 'Debug view');
    for (const view of DEBUG_VIEWS) debug.add(new Option(`debug: ${view}`, view));
    debug.addEventListener('change', () => (engine.view = { ...engine.view, debug: debug.value as DebugView }));
    element.replaceChildren(text, debug);
    addEventListener('keydown', event => {
        if (event.key.toLowerCase() === 'h' && !event.ctrlKey && !event.metaKey) element.hidden = !element.hidden;
    });
    const lab = Object.entries(labs).map(([name, variant]) => `${name}:${variant}`).join(' ');
    const update = () => {
        const stats = engine.stats(), view = engine.view;
        debug.value = view.debug;
        const passes = Object.entries(stats.gpu).map(([pass, ms]) => `${pass} ${ms.toFixed(2)} ms`).join(' · ') || 'no timestamp queries';
        const pools = [...stats.pools].map(([pool, bytes]) => `${pool} ${(bytes / MB).toFixed(1)}`).join(' · ');
        const height = view.mode === 'walk' ? `eye ${view.height.toFixed(1)} m` : `altitude ${Math.round(view.height)} m`;
        text.textContent = [
            `adapter ${stats.adapter} · lost devices recovered: ${stats.recoveries}`,
            `gpu   ${passes}`,
            `cpu   ${stats.cpu.toFixed(2)} ms`,
            `MB    ${pools}`,
            `net   ${stats.requests} requests · ${(stats.bytes / MB).toFixed(2)} MB${stats.failed ? ` · ${stats.failed} failed` : ''}`,
            `tiles ${stats.drawn} drawn · ${stats.resident} resident`,
            `lab   ${lab}`,
            `view  ${view.mode} · ${view.lat.toFixed(5)}, ${view.lon.toFixed(5)} · heading ${view.heading.toFixed(0)}° · pitch ${view.pitch.toFixed(0)}° · ${height}`,
            `light ${view.date} ${view.time} · ${view.weather}`,
            'keys  W A S D move · drag to look · F walk or fly · wheel altitude · Ctrl+C / Ctrl+V view · H HUD',
        ].join('\n');
    };
    update();
    setInterval(update, REFRESH_MS);
}
