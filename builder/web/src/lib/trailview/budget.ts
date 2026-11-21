/** The device class of a page, chosen once from the address (`tierOf` of view.ts): `phone` is the iOS app on every iOS device, `desktop` every other host. */
export type Tier = 'desktop' | 'phone';

export interface Budget {
    /** The frames per second that the engine draws while the view moves. */
    frameRate: number;
    /** The share of the terrain's page pools, after the feature masks take theirs. */
    terrainPages: number;
    /** The ground look's pages: whole layers of 16. */
    groundPages: number;
    /** Loads in flight at most: terrain tiles, ground tiles, house tiles, stamp jobs of the worker, canopy tiles. */
    loads: { terrain: number; ground: number; houses: number; stamps: number; canopy: number };
}

/**
 * The desktop's canopy loads are the 3 × 3 tiles about the eye at once. The phone's values are those of the
 * lever measurement's preset P on the iPhone 13: 64 ground pages are needed to load at all there.
 */
export const BUDGETS: Record<Tier, Budget> = {
    desktop: { frameRate: 60, terrainPages: 1, groundPages: 192, loads: { terrain: 6, ground: 4, houses: 6, stamps: 8, canopy: 9 } },
    phone: { frameRate: 30, terrainPages: 0.5, groundPages: 64, loads: { terrain: 3, ground: 2, houses: 3, stamps: 4, canopy: 5 } },
};
