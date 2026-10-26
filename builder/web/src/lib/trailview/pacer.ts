/** The frames per second that the engine draws while the view moves. */
export const FRAME_RATE = 60;
/** Display frames whose median length is the refresh period. */
const PERIODS = 8;
/** A gap longer than this is an idle engine, not a display frame. */
const IDLE_MS = 100;

/**
 * Picks the display frames that draw, about `rate` per second: the first display frame at or after
 * 1 / rate, less half a refresh period, since the last frame drawn. A display whose refresh rate is no
 * multiple of `rate` draws at the nearest even step, 72 frames per second for 60 on 144 Hz: an uneven
 * cadence would judder. A quality controller may change `rate` at run time.
 */
export class Pacer {
    rate = FRAME_RATE;
    private drawn = -Infinity;
    private previous?: number;
    private readonly periods: number[] = [];

    /** Whether the display frame at `now` (milliseconds) draws; with `force` it draws. */
    due(now: number, force = false): boolean {
        const gap = now - (this.previous ?? Infinity);
        this.previous = now;
        if (gap > 0 && gap < IDLE_MS) {
            this.periods.push(gap);
            if (this.periods.length > PERIODS) this.periods.shift();
        }
        const period = [...this.periods].sort((a, b) => a - b)[this.periods.length >> 1] ?? 0;
        if (!force && now - this.drawn < 1000 / this.rate - period / 2) return false;
        this.drawn = now;
        return true;
    }
}
