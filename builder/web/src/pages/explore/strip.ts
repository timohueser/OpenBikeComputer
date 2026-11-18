/** The layout of the strip of stages on the trail card: one cell for each part of the trail, in trail order. */
import type { TrailPart } from '../../lib/planner/signed-routes';

export interface StripCell {
    id: number;
    /** The share of the width of the strip, from 0 to 1. */
    share: number;
    /** The official stage number, on a trail of two or more stages. */
    number?: number;
    missing: boolean;
    /** A clear gap before the cell. */
    jump: boolean;
}

/** A part is as wide as its length, the difference of its `end_m` to the part before. A part with no length is as wide
 * as the mean of the parts with one. */
export function stripCells(parts: TrailPart[], numbered: boolean): StripCell[] {
    const lengths = parts.map((part, i) => part.end_m - (parts[i - 1]?.end_m ?? 0));
    const known = lengths.filter(length => length > 0);
    const mean = known.length ? known.reduce((sum, length) => sum + length, 0) / known.length : 1;
    const widths = lengths.map(length => length > 0 ? length : mean), total = widths.reduce((sum, width) => sum + width, 0);
    return parts.map((part, i) => ({
        id: part.id, share: widths[i] / total, ...numbered && part.stage !== undefined ? { number: part.stage } : {},
        missing: !!part.missing, jump: !!part.jump,
    }));
}
