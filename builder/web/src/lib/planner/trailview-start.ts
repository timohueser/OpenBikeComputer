// The entry of the engine's chunk: only `loadTrailView()` imports it.
import { TrailView } from '../trailview/engine';
import { Gpu } from '../trailview/gpu/device';
import { scaleOf } from '../trailview/resolve/targets';
import type { View } from '../trailview/view';
import type { PlannerConfig } from './config';
import { firstView, type Near } from './trailview-first';

let device: Promise<Gpu | undefined> | undefined;
/**
 * The page's one GPU device. The binoculars ask for it at a press, as a cold device takes over a second, and later
 * opens use it again. A lost device is asked for again at the next press.
 */
export function requestGpu(): Promise<Gpu | undefined> {
    return device ??= Gpu.request().then(gpu => {
        void gpu?.device.lost.then(() => { device = undefined; });
        return gpu;
    }, (error: unknown) => {
        device = undefined;
        throw error;
    });
}

/**
 * The engine on the canvas, or undefined when the browser has no WebGPU adapter. It starts at `view`
 * and moves to the first view of the drop, which loads while the engine compiles.
 */
export async function startTrailView(canvas: HTMLCanvasElement, config: PlannerConfig, view: View, near: Near): Promise<TrailView | undefined> {
    // The first view cannot reject: a throw after the start would leave a running engine that no caller holds.
    const first = firstView(config, [view.lon, view.lat], near).catch((error: unknown) => void console.info('TrailView first view falls back to the drop:', error));
    const gpu = await requestGpu();
    if (!gpu) return undefined;
    const engine = await TrailView.start(canvas, gpu, config, view, scaleOf(''));
    const moved = await first;
    if (moved) engine.view = { ...engine.view, ...moved };
    return engine;
}

/** The longest wait for the near field after the first frame: a slow network shows the view still refining. */
const SETTLE_CAP_MS = 3_000;

/**
 * Whether the engine shows a settled view: true once the near field is final (`nearReady()`), or
 * SETTLE_CAP_MS after the first frame with terrain. False when a load failed
 * and nothing is drawn after 5 s, when nothing is drawn after 30 s, or when `signal` aborts. The first
 * frames show coarse ground and an eye that still moves to it, so the window hides them.
 */
export async function viewSettled(engine: TrailView, signal: AbortSignal): Promise<boolean> {
    const start = performance.now();
    while (!engine.stats().drawn) {
        const waited = performance.now() - start;
        if (signal.aborted || waited > 30_000 || (engine.stats().failed && waited > 5_000)) return false;
        await new Promise(resolve => setTimeout(resolve, 200));
    }
    const aborted = new Promise<void>(resolve => signal.addEventListener('abort', () => resolve(), { once: true }));
    await Promise.race([engine.nearReady(), new Promise(resolve => setTimeout(resolve, SETTLE_CAP_MS)), aborted]);
    return !signal.aborted;
}

/** Ends the engine and frees its GPU memory and its workers; the page's device stays for the next open. */
export function stopTrailView(engine: TrailView): void {
    engine.destroy();
}
