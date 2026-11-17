/** The height profile of a route as `profile_m` of the route catalog has it (`specs/route-catalog.md`, "Profile"). */

/** A height in metres at each `step` metres along a route of `length` metres, and one at its end; null is unknown. */
export interface Profile { step: number; heights: readonly (number | null)[]; length: number }

/** Metres along the route of entry `i`: i × step, and the last entry at the end. */
export function profileEntry({ step, heights, length }: Profile, i: number): number {
    return i >= heights.length - 1 ? length : Math.min(i * step, length);
}

/** The height at `metres` along the route, on the straight line between the two entries about it: null where one of them has none. */
export function profileHeight(profile: Profile, metres: number): number | null {
    const { step, heights } = profile, i = Math.max(0, Math.min(Math.floor(metres / step), heights.length - 2));
    // A profile of one entry has no next entry.
    const a = heights[i], b = i + 1 < heights.length ? heights[i + 1] : a;
    const from = profileEntry(profile, i), span = profileEntry(profile, i + 1) - from;
    // Two entries at one place (the end of a route whose length is a multiple of the step): the later one wins.
    const t = span > 0 ? Math.max(0, Math.min(1, (metres - from) / span)) : 1;
    return t === 0 ? a : t === 1 ? b : a === null || b === null ? null : a + (b - a) * t;
}
