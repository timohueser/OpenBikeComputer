/** Where route progress sits across the elevation drawer: a linear scale over the shown stretch. */
export interface ProfileAxis {
    from: number;
    to: number;
}

const share = ({ from, to }: ProfileAxis, progress: number) => (progress - from) / (to - from);

/** The CSS position of `progress`. */
export function axisLeft(axis: ProfileAxis, progress: number): string {
    return `${share(axis, progress) * 100}%`;
}

/** The position of `progress` in px, on a drawer track `width` px wide. */
export function axisPixels(axis: ProfileAxis, progress: number, width: number): number {
    return share(axis, progress) * width;
}

/** The CSS width from `start` to `end`. */
export function axisWidth(axis: ProfileAxis, start: number, end: number): string {
    return `${(end - start) / (axis.to - axis.from) * 100}%`;
}

/** The progress `x` px into a drawer track `width` px wide, held inside the shown stretch. */
export function axisProgress(axis: ProfileAxis, x: number, width: number): number {
    return Math.max(axis.from, Math.min(axis.to, axis.from + x / Math.max(1e-9, width) * (axis.to - axis.from)));
}
