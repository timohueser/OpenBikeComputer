/** Where route progress sits across the elevation drawer: a linear scale over the shown stretch, with a column at each
 * rest day inside it. A column is 64 px wide, but the columns together take at most half the track. Positions are CSS
 * lengths, so the columns keep their width at any drawer width. */
export interface ProfileAxis {
    from: number;
    to: number;
    /** The progress of each rest day inside the shown stretch, in order. */
    rests: number[];
}

const restWidth = 64;

/** The CSS width of one rest column. */
export function axisColumn({ rests }: ProfileAxis): string {
    return rests.length ? `min(${restWidth}px, ${50 / rests.length}%)` : '0px';
}

/** The width of one rest column in px, on a drawer track `width` px wide. */
function columnPixels({ rests }: ProfileAxis, width: number): number {
    return rests.length ? Math.min(restWidth, width / 2 / rests.length) : 0;
}

/** The CSS position of `progress`. At a rest, `after` gives the far side of its column, where the next day starts. */
export function axisLeft(axis: ProfileAxis, progress: number, after = false): string {
    const share = (progress - axis.from) / (axis.to - axis.from);
    const before = axis.rests.filter(rest => after ? rest <= progress : rest < progress).length;
    return `calc(${share * 100}% + ${before - share * axis.rests.length} * ${axisColumn(axis)})`;
}

/** The CSS position of the column of `axis.rests[index]`; two rests after one night stand side by side. */
export function axisRest(axis: ProfileAxis, index: number): string {
    const share = (axis.rests[index] - axis.from) / (axis.to - axis.from);
    return `calc(${share * 100}% + ${index - share * axis.rests.length} * ${axisColumn(axis)})`;
}

/** The position of `progress` in px, on a drawer track `width` px wide. */
export function axisPixels(axis: ProfileAxis, progress: number, width: number): number {
    const share = (progress - axis.from) / (axis.to - axis.from), column = columnPixels(axis, width);
    return share * (width - axis.rests.length * column) + axis.rests.filter(rest => rest < progress).length * column;
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

/** The progress `x` px into a drawer track `width` px wide; a point in a rest column is at its rest. The columns of a
 * `moving` rest follow the pointer, as when the rider drags its day end, so they take no width before it. */
export function axisProgress(axis: ProfileAxis, x: number, width: number, moving?: number): number {
    const column = columnPixels(axis, width);
    const perProgress = Math.max(1e-9, width - axis.rests.length * column) / (axis.to - axis.from);
    let left = 0, start = axis.from;
    for (const rest of axis.rests.filter(rest => rest !== moving)) {
        const right = left + (rest - start) * perProgress;
        if (x <= right) break;
        if (x <= right + column) return rest;
        left = right + column;
        start = rest;
    }
    return Math.max(axis.from, Math.min(axis.to, start + (x - left) / perProgress));
}
