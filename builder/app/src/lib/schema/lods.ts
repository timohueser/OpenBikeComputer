import type { LodTier } from "../config/model";

/** A stable scale that dispatches to `index` under Reader::select_lod_for_mpp. */
export function representativeMpp(lods: LodTier[], index: number): number {
    if (index <= 0) {
        const next = lods[1]?.max_mpp;
        return typeof next === "number" && Number.isFinite(next) ? Math.min(100, next + Math.max(1, next * 0.2)) : 40;
    }
    const ceiling = lods[index]?.max_mpp;
    if (typeof ceiling === "number" && Number.isFinite(ceiling) && ceiling > 0) return ceiling;
    return Math.max(0.5, 40 / 2 ** index);
}
