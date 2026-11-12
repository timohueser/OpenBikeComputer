import type { PlannerConfig } from '../planner/config';
import { TrailView } from './engine';
import type { Gpu } from './gpu/device';
import { MAX_SCALE } from './resolve/targets';
import type { View } from './view';

/**
 * The place of each point of a line in a still. `x` and `y` are pixels of the image and can lie outside
 * it; `distance` is metres from the eye. `visible` is 0 for a point that is hidden, outside the image,
 * behind the eye or on terrain with no data. Behind the eye and with no data, `x`, `y` and `distance`
 * are NaN.
 */
export interface StillPoints { x: Float32Array; y: Float32Array; distance: Float32Array; visible: Uint8Array }

/** A view (with `debug: 'off'`) to draw at a size in pixels and, for a cover, a line of [lon, lat] points. */
export interface StillJob { view: View; width: number; height: number; line?: [number, number][] }

export interface Still { image: Blob; points?: StillPoints }

/** Why a still failed; after `device-lost` the caller asks for a new `Gpu`. */
export type StillFailure = 'aborted' | 'timeout' | 'hidden' | 'device-lost' | 'tiles-failed';

export class StillError extends Error {
    declare name: StillFailure;

    constructor(name: StillFailure, message: string) {
        super(message);
        this.name = name;
    }
}

const TIMEOUT_MS = 60_000;
/** WebP, or JPEG in a browser that does not encode WebP, such as Safari. */
const QUALITY = 0.85;

/**
 * Draws one view to a still with an engine of its own, on a canvas that the page does not show, and
 * ends the engine after it. The caller makes sure that no other engine draws at that time: an engine
 * holds hundreds of megabytes of GPU memory. A tile that fails fails the still, so no still has holes.
 */
export async function still(gpu: Gpu, config: PlannerConfig, job: StillJob, { signal, timeoutMs = TIMEOUT_MS }: { signal?: AbortSignal; timeoutMs?: number } = {}): Promise<Still> {
    const done = new AbortController();
    const failed = new Promise<never>((_, reject) => {
        const fail = (name: StillFailure, message: string) => reject(new StillError(name, message));
        const timer = setTimeout(() => fail('timeout', `The still took more than ${timeoutMs} ms.`), timeoutMs);
        done.signal.addEventListener('abort', () => clearTimeout(timer));
        // A hidden page draws no frame.
        const hidden = () => document.hidden && fail('hidden', 'The page became hidden.');
        const aborted = () => signal?.aborted && fail('aborted', 'The still was aborted.');
        hidden();
        aborted();
        document.addEventListener('visibilitychange', hidden, { signal: done.signal });
        signal?.addEventListener('abort', aborted, { signal: done.signal });
        void gpu.device.lost.then(info => fail('device-lost', `The WebGPU device was lost: ${info.message}`));
    });
    const started = TrailView.start(document.createElement('canvas'), gpu, config, job.view, MAX_SCALE, [job.width, job.height]);
    try {
        return await Promise.race([failed, started.then(async engine => {
            await engine.waitForIdle();
            const tiles = engine.stats().failed;
            if (tiles) throw new StillError('tiles-failed', `${tiles} tiles failed to load.`);
            const { colour, points } = await engine.capture(job.line);
            return { image: await encode(colour), points };
        })]);
    } finally {
        done.abort();
        // A start that ends after a failure ends its engine too.
        started.then(engine => engine.destroy(), () => {});
    }
}

async function encode(colour: ImageData): Promise<Blob> {
    const canvas = new OffscreenCanvas(colour.width, colour.height);
    canvas.getContext('2d')!.putImageData(colour, 0, 0);
    // A browser that cannot encode a type gives PNG.
    const webp = await canvas.convertToBlob({ type: 'image/webp', quality: QUALITY });
    return webp.type === 'image/webp' ? webp : canvas.convertToBlob({ type: 'image/jpeg', quality: QUALITY });
}

/**
 * Whether this browser renders stills on a GPU: false with no adapter, and with a software adapter,
 * whose still takes minutes. `software` lets a script render on one.
 */
export async function canRenderStills({ software = false } = {}): Promise<boolean> {
    const adapter = await navigator.gpu?.requestAdapter();
    if (!adapter) return false;
    const { isFallbackAdapter, vendor, architecture, description } = adapter.info;
    return software || !(isFallbackAdapter || /swiftshader/i.test(`${vendor} ${architecture} ${description}`));
}
