/**
 * The device class of a page, chosen once from the address (`tierOf` of view.ts): `tier=phone` chooses `phone`, the tier
 * for the iOS app. Every other page is `desktop`, the planner's TrailView too.
 */
export type Tier = 'desktop' | 'phone';

export interface Budget {
    /** The frames per second that the engine draws while the view moves. */
    frameRate: number;
    /** The share of the terrain's page pools, 0 to 1, after the feature masks take theirs. */
    terrainShare: number;
    /** The ground look's pages: whole layers of 16. */
    groundPages: number;
    /** Loads in flight at most: terrain tiles, ground tiles, house tiles, stamp jobs of the worker, canopy tiles. */
    loads: { terrain: number; ground: number; houses: number; stamps: number; canopy: number };
    /**
     * The terrain's CDLOD range in node sizes, and its morph band in levels of log2 distance (`cdlod` of lod.ts). Below the
     * least, a node can reach past the morph band of its parent's level, and neighbours would not meet (lod.test.ts); a
     * narrower band lets them meet at a lower least. Above the most, a level leaves its LOD map window.
     */
    lod: { least: number; most: number; morph: number };
}

/**
 * The desktop's canopy loads are the 3 × 3 tiles about the eye at once. An iPhone 13 does not load a view with more
 * than 64 ground pages. The phone's least and most range are equal: its range does not follow the render size
 * (`bestRange` of terrain.ts), and no quality step lowers it.
 */
export const BUDGETS: Record<Tier, Budget> = {
    desktop: { frameRate: 60, terrainShare: 1, groundPages: 192, loads: { terrain: 6, ground: 4, houses: 6, stamps: 8, canopy: 9 }, lod: { least: 4.6, most: 8, morph: 0.3 } },
    phone: { frameRate: 30, terrainShare: 0.5, groundPages: 64, loads: { terrain: 3, ground: 2, houses: 3, stamps: 4, canopy: 5 }, lod: { least: 3.7, most: 3.7, morph: 0.15 } },
};
