/** The ticks under the rows of a detail page: round distances in the unit of the rider, and the ticks that fit. */
import { distanceIn } from '../../lib/settings/format';
import { settings } from '../../lib/settings/settings.svelte';

export interface Tick { at: number; label: string }

/** A tick label is about TICK_CHAR_PX wide for each character, and keeps TICK_GAP_PX from the next one. */
const TICK_CHAR_PX = 7, TICK_GAP_PX = 12;

/** Ticks at round distances from `fromM` to `toM` along a line, about one for each 80 px of `width`. */
export function distanceTicks(fromM: number, toM: number, width: number): Tick[] {
    const first = distanceIn(fromM), length = distanceIn(toM - fromM);
    const step = [1, 2, 5, 10, 20, 50, 100, 200, 500, 1000].find(size => length / size <= Math.max(1, width / 80)) ?? 2000;
    const ticks = [];
    for (let value = Math.ceil(first / step) * step; value < first + length; value += step) ticks.push({ at: (value - first) / length, label: `${settings.units} ${value}` });
    return ticks;
}

/** The ticks of `wanted` that fit on `width` pixels, the first ones first: a tick that would touch a kept one is left
 * out. A label sits as `Lanes` places it: shifted left by its share of the axis. */
export function fitTicks(wanted: Tick[], width: number): Tick[] {
    const extent = ({ at, label }: Tick) => {
        const size = label.length * TICK_CHAR_PX;
        return [at * width - at * size, at * width + (1 - at) * size];
    };
    const kept: Tick[] = [];
    for (const tick of wanted) {
        const [left, right] = extent(tick);
        if (kept.every(other => { const [a, b] = extent(other); return right + TICK_GAP_PX <= a || left >= b + TICK_GAP_PX; })) kept.push(tick);
    }
    return kept;
}
