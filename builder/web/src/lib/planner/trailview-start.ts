// The entry of the engine's chunk: only `loadTrailView()` imports it.
import { TrailView } from '../trailview/engine';
import { Gpu } from '../trailview/gpu/device';
import { scaleOf } from '../trailview/resolve/targets';
import type { View } from '../trailview/view';
import type { PlannerConfig } from './config';

/** The engine on the canvas, or undefined when the browser has no WebGPU adapter. */
export async function startTrailView(canvas: HTMLCanvasElement, config: PlannerConfig, view: View): Promise<TrailView | undefined> {
    const gpu = await Gpu.request();
    return gpu && TrailView.start(canvas, gpu, config, view, scaleOf(''));
}

/** Resolves after the first frame that draws terrain, or when `signal` aborts. */
export async function groundDrawn(engine: TrailView, signal: AbortSignal): Promise<void> {
    while (!signal.aborted && !engine.stats().drawn) await engine.nextFrame();
}

/** The engine frees its GPU memory and its workers in destroy(). */
export function stopTrailView(engine: TrailView): void {
    (engine as { destroy?: () => void }).destroy?.();
}
