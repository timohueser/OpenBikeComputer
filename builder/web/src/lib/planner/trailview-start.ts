// The entry of the engine's chunk: only `loadTrailView()` imports it.
import { TrailView } from '../trailview/engine';
import { Gpu } from '../trailview/gpu/device';
import { scaleOf } from '../trailview/resolve/targets';
import type { View } from '../trailview/view';
import type { PlannerConfig } from './config';
import { firstView, type Near } from './trailview-first';

/**
 * The engine on the canvas, or undefined when the browser has no WebGPU adapter. It starts at `view`
 * and moves to the first view of the drop, which loads while the engine compiles.
 */
export async function startTrailView(canvas: HTMLCanvasElement, config: PlannerConfig, view: View, near: Near): Promise<TrailView | undefined> {
    const first = firstView(config, [view.lon, view.lat], near);
    const gpu = await Gpu.request();
    if (!gpu) return undefined;
    const engine = await TrailView.start(canvas, gpu, config, view, scaleOf(''));
    engine.view = { ...engine.view, ...(await first) };
    return engine;
}

/**
 * Whether the engine draws terrain: false when a load failed and nothing is drawn after 5 s, when nothing is
 * drawn after 30 s, or when `signal` aborts. It polls: the engine draws by itself as the tiles arrive.
 */
export async function groundDrawn(engine: TrailView, signal: AbortSignal): Promise<boolean> {
    const start = performance.now();
    while (!signal.aborted) {
        const { drawn, failed } = engine.stats(), waited = performance.now() - start;
        if (drawn) return true;
        if (waited > 30_000 || (failed && waited > 5_000)) return false;
        await new Promise(resolve => setTimeout(resolve, 200));
    }
    return false;
}

/** The engine frees its GPU memory and its workers in destroy(). */
export function stopTrailView(engine: TrailView): void {
    (engine as { destroy?: () => void }).destroy?.();
}
