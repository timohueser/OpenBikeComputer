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
     * Byte caps in MiB. They are not hard limits: a decode in flight stays, and the stamp worker keeps the tiles that a
     * view wants past its cap. Worker caps: the decoded images of each ground worker, the feature tiles of the stamp and
     * the house worker, and the heap of a plan worker, past which it starts again. Main page caps: the stamped nodes, the
     * records of the plan's debug draw (`plan=faces`), and the encoded bodies of the terrain tiles, of the feature tiles
     * and of each ground archive.
     */
    caps: {
        groundDecode: number; stamp: number; house: number; planHeap: number;
        nodes: number; debugRecords: number; terrainBodies: number; featureBodies: number; groundBodies: number;
    };
    /** The plan workers of the ground plan (`ground=plan`). */
    planWorkers: number;
}

export const MiB = 2 ** 20;

/**
 * The desktop's canopy loads are the 3 × 3 tiles about the eye at once. An iPhone 13 does not load a view with more
 * than 64 ground pages.
 */
export const BUDGETS: Record<Tier, Budget> = {
    desktop: {
        frameRate: 60, terrainShare: 1, groundPages: 192, loads: { terrain: 6, ground: 4, houses: 6, stamps: 8, canopy: 9 },
        caps: { groundDecode: 96, stamp: 48, house: 32, planHeap: 55, nodes: 64, debugRecords: 32, terrainBodies: 64, featureBodies: 32, groundBodies: 16 },
        planWorkers: 2,
    },
    phone: {
        frameRate: 30, terrainShare: 0.5, groundPages: 64, loads: { terrain: 3, ground: 2, houses: 3, stamps: 4, canopy: 5 },
        caps: { groundDecode: 16, stamp: 24, house: 16, planHeap: 40, nodes: 24, debugRecords: 16, terrainBodies: 16, featureBodies: 8, groundBodies: 2 },
        planWorkers: 1,
    },
};

/**
 * The phone's byte caps added up, in MiB. The owner chose the phone's caps so that the page stays under 896 MiB on an
 * iPhone 13: iOS ends a page there at about 1,408 MiB, and a load keeps 512 MiB of headroom. A change that raises a cap
 * raises this sum and says why; the phone runs measure the memory that the caps leave.
 */
export const PHONE_CAPS = 188;
