/** The jittered frames that a still view takes: the finished image is their mean. */
export const STILL_FRAMES = 16;
/** The weight of a new frame against the reprojected history while the camera moves. */
export const MOVING_WEIGHT = 0.1;

/** Element `i` (from 1) of the Halton sequence of a base, in [0, 1). */
export function halton(i: number, base: number): number {
    let f = 1, r = 0;
    for (; i > 0; i = Math.floor(i / base)) {
        f /= base;
        r += f * (i % base);
    }
    return r;
}

/** The jitter of sample k (from 1) in render pixels, each axis in [-0.5, 0.5): Halton 2, 3. */
export const jitter = (k: number): [number, number] => [halton(k, 2) - 0.5, halton(k, 3) - 0.5];

/** What a frame draws: the weight of the new frame against the history, its jitter sample, and whether the history is clipped to the frame. */
export interface Step {
    weight: number;
    sample: number;
    clip: boolean;
}

/**
 * The accumulation of jittered frames. While the camera moves, a frame weighs MOVING_WEIGHT against
 * the reprojected history, clipped to the frame. When the camera stops, and when the content of a
 * still view changes, it starts again: frame k weighs 1/k, so after `frames` frames the image is their
 * mean, whatever path led to the view, and the view needs no more frames.
 */
export class Temporal {
    /** Still frames since the start; 0 while the camera moves. */
    private k = 0;
    private moving = 0;
    private valid = false;
    private frames = STILL_FRAMES;

    /** Whether a still view needs more frames. */
    get converging(): boolean {
        return this.k < this.frames;
    }

    /** The history is lost: the next frame starts it again. */
    reset(): void {
        this.valid = false;
        this.k = 0;
    }

    /** The step of the next frame, or undefined when the still view has all its frames. `frames` is the number that a still view takes. */
    next(moved: boolean, changed: boolean, frames = STILL_FRAMES): Step | undefined {
        this.frames = frames;
        if (moved) {
            this.k = 0;
            this.moving = this.moving % STILL_FRAMES + 1;
            const weight = this.valid ? MOVING_WEIGHT : 1;
            this.valid = true;
            return { weight, sample: this.moving, clip: true };
        }
        if (changed || !this.valid) this.k = 0;
        if (this.k >= frames) return undefined;
        this.k++;
        this.valid = true;
        return { weight: 1 / this.k, sample: this.k, clip: false };
    }
}
