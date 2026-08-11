import type { LodTier } from "../config/model";

/**
 * The ladder the LOD chips must describe: the one the **displayed** map was packed with, not
 * the one being edited. The two differ for the whole debounce-plus-pack window (seconds), and a
 * chip for a tier the loaded map does not carry can never light — clicking it just dispatches the
 * renderer to some other tier. `lodCount` is the loaded map's own table size, read straight off
 * the render stats, and is the tiebreak when the two disagree for any other reason.
 *
 * `packed` is null before the first successful pack, and then the editable config is all there is
 * to show.
 */
export function previewLadder(packed: LodTier[] | null, editing: LodTier[], lodCount?: number): LodTier[] {
    const source = packed ?? editing;
    if (lodCount === undefined || lodCount === source.length) return source;
    // The loaded map's own table wins on how many tiers exist; a tier it carries that neither
    // ladder describes falls back to the bridge's default scale rather than disappearing.
    return Array.from({ length: lodCount }, (_, i) => source[i] ?? { max_mpp: null, simplify: 0 });
}

/** A stable scale that dispatches to `index` under Reader::select_lod_for_mpp. */
export function representativeMpp(lods: LodTier[], index: number): number {
    if (index <= 0) {
        const next = lods[1]?.max_mpp;
        return typeof next === "number" && Number.isFinite(next) ? next + Math.max(1, next * 0.2) : 40;
    }
    const ceiling = lods[index]?.max_mpp;
    if (typeof ceiling === "number" && Number.isFinite(ceiling) && ceiling > 0) return ceiling;
    return Math.max(0.5, 40 / 2 ** index);
}
