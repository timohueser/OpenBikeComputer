// The trees' year. Each tree passes four turns: leaf-out, full green, autumn colour and leaf fall. A
// stand passes a turn over `days` about its `day`, and each tree on a day of its own, by its hash: the
// share of trees past a turn rises with the date, and a tree passes the turns in their order and never
// turns back. Higher up, spring comes later and autumn earlier.

/** The phases between the turns, in the order of SEASONS: winter (bare), spring, summer, autumn. */
export const WINTER = 0, SUMMER = 2;

/**
 * As TURNS of phenology.wgsl, per turn: its day of the year at REFERENCE_M (0 is 1 January), the days
 * over which a stand passes it, and the days that it comes later per metre higher. A stand passes
 * each turn before any of its trees pass the next, but above about 2320 m it is still passing full
 * green when its first trees pass autumn colour.
 */
export const TURNS: readonly (readonly [number, number, number])[] = [
    // Leaf-out, about 20 April.
    [110, 30, 0.03],
    // Full green, about 4 June.
    [155, 30, 0.03],
    // Autumn colour, about 29 September.
    [272, 40, -0.015],
    // Leaf fall, about 4 November.
    [308, 30, -0.015],
];
/** The elevation of the turns' days, and the highest trees: no tree turns as if it stood higher. */
export const REFERENCE_M = 500, TREELINE_M = 2500;

/** The share of the trees at an elevation that have passed turn k on a day, as `turned` of phenology.wgsl. */
export function turned(k: number, day: number, elevation: number): number {
    const [at, days, later] = TURNS[k];
    return Math.min(1, Math.max(0, (day - at - later * (Math.min(Math.max(elevation, 0), TREELINE_M) - REFERENCE_M)) / days + 0.5));
}

/** The phase of a tree whose hash is `u` (0 to 1), as `phase_of` of phenology.wgsl. */
export function phaseOf(u: number, day: number, elevation: number): number {
    let passed = 0;
    for (let k = 0; k < TURNS.length; k++) if (u < turned(k, day, elevation)) passed++;
    return passed % 4;
}

/** The share of the trees in each phase at an elevation, winter first. */
export function phaseShares(day: number, elevation: number): [number, number, number, number] {
    const [a, b, c, d] = [0, 1, 2, 3].map(k => turned(k, day, elevation));
    return [1 - a + d, a - b, b - c, c - d];
}

/** The phases that a tree from sea level to the tree line has on a day. */
export function phasesOn(day: number): Set<number> {
    const phases = new Set<number>();
    // A step of 50 m moves a turn by less than a day; its window is a month wide.
    for (let elevation = 0; elevation <= TREELINE_M; elevation += 50) {
        phaseShares(day, elevation).forEach((share, phase) => share > 0 && phases.add(phase));
    }
    return phases;
}
