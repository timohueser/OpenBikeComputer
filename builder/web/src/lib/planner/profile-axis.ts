/** Where route progress sits across the elevation drawer: a linear scale over the shown stretch, with a column of fixed
 * width at each rest day inside it. Positions are CSS lengths, so a column keeps its width at any drawer width. */
export interface ProfileAxis {
    from: number;
    to: number;
    /** The progress of each rest day inside the shown stretch, in order. */
    rests: number[];
}

export const restWidth = 64;

/** The CSS position of `progress`. At a rest, `after` gives the far side of its column, where the next day starts. */
export function axisLeft({ from, to, rests }: ProfileAxis, progress: number, after = false): string {
    const share = (progress - from) / (to - from);
    const before = rests.filter(rest => after ? rest <= progress : rest < progress).length;
    return `calc(${share * 100}% + ${(before - share * rests.length) * restWidth}px)`;
}

/** The position of `progress` in px, on a drawer track `width` px wide. */
export function axisPixels({ from, to, rests }: ProfileAxis, progress: number, width: number): number {
    const share = (progress - from) / (to - from);
    return share * (width - rests.length * restWidth) + rests.filter(rest => rest < progress).length * restWidth;
}

/** The CSS width from `start` to `end`; a stretch that starts at a rest starts after its column. */
export function axisWidth(axis: ProfileAxis, start: number, end: number): string {
    return `calc(${axisLeft(axis, end)} - ${axisLeft(axis, start, true)})`;
}

/** The stretches between the rest columns. */
export function axisPieces({ from, to, rests }: ProfileAxis): [number, number][] {
    const ends = [from, ...rests, to];
    return ends.slice(1).map((end, i) => [ends[i], end]);
}

/** The progress `x` px into a drawer track `width` px wide; a point in a rest column is at its rest. */
export function axisProgress(axis: ProfileAxis, x: number, width: number): number {
    const perProgress = Math.max(1e-9, width - axis.rests.length * restWidth) / (axis.to - axis.from);
    let left = 0;
    for (const [start, end] of axisPieces(axis)) {
        const right = left + (end - start) * perProgress;
        if (x <= right || end === axis.to) return Math.max(axis.from, Math.min(axis.to, start + (x - left) / perProgress));
        if (x <= right + restWidth) return end;
        left = right + restWidth;
    }
    return axis.to;
}
