/** The frames per second that the engine draws while the view moves. */
export const FRAME_RATE = 60;
/** Display frames whose median length is the refresh period. */
const PERIODS = 8;
/** A gap longer than this is an idle engine, not a display frame. */
const IDLE_MS = 100;
/** How far past a half the display frames per drawn frame must move before the step changes: a ratio such as 1.5 does not flip. */
const HYSTERESIS = 0.15;

/**
 * Picks the display frames that draw, about `rate` (> 0) per second: every `step`th display frame, the
 * whole number of display frames nearest to the refresh rate over `rate`. 144 Hz at 60 draws 72 frames
 * per second: an uneven cadence would judder. A frame draws half a refresh period early at the latest,
 * so a late or early timestamp does not skip a frame. A quality controller may change `rate` at run time.
 */
export class Pacer {
    rate = FRAME_RATE;
    private step = 1;
    private drawn = -Infinity;
    private previous?: number;
    private readonly periods: number[] = [];

    /** Whether the display frame at `now` (milliseconds) draws. */
    due(now: number): boolean {
        const gap = now - (this.previous ?? Infinity);
        this.previous = now;
        if (gap > 0 && gap < IDLE_MS) {
            this.periods.push(gap);
            if (this.periods.length > PERIODS) this.periods.shift();
        }
        const period = [...this.periods].sort((a, b) => a - b)[this.periods.length >> 1] ?? 0;
        const ratio = period && 1000 / this.rate / period;
        if (Math.abs(ratio - this.step) > 0.5 + HYSTERESIS) this.step = Math.max(1, Math.round(ratio));
        if (now - this.drawn < (this.step - 0.5) * period) return false;
        this.drawn = now;
        return true;
    }
}
