import type { Bbox } from "./regionPicker";

/** What the map panel hands the build flow: one area, either way of picking it. */
export interface AreaSelection {
    mode: "regions" | "bbox";
    /** Selected region ids (regions mode). */
    regionIds: string[];
    regionNames: string[];
    /** The drawn box + the source regions covering it (bbox mode). */
    bbox: Bbox | null;
    coveringIds: string[];
    coveringNames: string[];
    areaKm2: string | null;
}

export function emptySelection(): AreaSelection {
    return {
        mode: "regions",
        regionIds: [],
        regionNames: [],
        bbox: null,
        coveringIds: [],
        coveringNames: [],
        areaKm2: null,
    };
}

/** The region ids a build request needs (source regions in bbox mode). */
export function buildRegionIds(sel: AreaSelection): string[] {
    return sel.mode === "bbox" ? sel.coveringIds : sel.regionIds;
}

export function selectionReady(sel: AreaSelection): boolean {
    return sel.mode === "bbox" ? sel.bbox !== null && sel.coveringIds.length > 0 : sel.regionIds.length > 0;
}
