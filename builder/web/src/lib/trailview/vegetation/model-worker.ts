// Builds tree models off the main thread: three tiers of each, as plain arrays.
import { SEASONS, type Season } from '../rules/regions';
import { bareIn, tree } from '../models/library';
import { lit, simplified, simplifierReady } from '../models/simplify';
import { fits, tone } from '../models/tone';
import type { Grown, Lod } from '../models/trees';
import { SUMMER } from './phenology';

/**
 * The crown triangles of tiers 1 and 2, which the simplifier makes from tier 0. A bare crown's tiers
 * grow from the same tree (trees.ts).
 */
const TRIANGLES = [600, 200];
/** A phase draws the summer crown through a colour matrix where that turns every crown vertex within this many sRGB steps of 255. */
const FIT = 8;

/**
 * A job: species to build in summer, kept green all year or not, with the crowns of their own that
 * `phases` need, and crowns of a species in a phase.
 */
export interface ModelJob { id: number; leafy: { model: string; evergreen: boolean }[]; phases: number[]; crowns: { model: string; phase: number }[] }
export interface BuiltTier { position: Float32Array; normal: Float32Array; color: Float32Array; index: Uint32Array }
/** A model's tiers, each with its trunk's vertices first, and its crown's lowest point in model units at every tier. */
export interface BuiltModel { tiers: (BuiltTier & { trunk: number })[]; crown: number }
/** The crown of a species in a phase. */
export interface BuiltCrown extends BuiltModel { model: string; phase: number }
/** A job's answer: the summer crowns in the order of its `leafy`, and every crown that it built. */
export interface ModelAnswer { id: number; leafy?: BuiltLeafy[]; crowns?: BuiltCrown[]; error?: string }
/**
 * A species' leafy crown in summer, with per phase (winter first) the colour matrix (3 × 3, columns
 * first) of the least error that turns it into that phase's, and whether the phase needs a crown of its
 * own: a bare crown, or flecks that the season moves, which the matrix does not turn within FIT.
 */
export interface BuiltLeafy extends BuiltModel { tones: (number[] | undefined)[]; own: boolean[] }

self.onmessage = async ({ data: { id, leafy, phases, crowns } }: MessageEvent<ModelJob>) => {
    try {
        await simplifierReady;
        const species = leafy.map(({ model, evergreen }): BuiltLeafy => {
            const summer = ([0, 1, 2] as Lod[]).map(lod => tree(model, 'summer', lod));
            const tones: (number[] | undefined)[] = [], own: boolean[] = [];
            SEASONS.forEach((season, phase) => {
                if (evergreen || phase === SUMMER) return;
                const near = bareIn(model, season) ? undefined : tree(model, season, 0), matrix = near && tone(summer[0], near);
                tones[phase] = matrix;
                own[phase] = !matrix || !fits(matrix, summer[0], near!, FIT);
            });
            return { ...arrays(tiers(summer)), tones, own };
        });
        const own = leafy.flatMap(({ model }, i) => phases.filter(phase => species[i].own[phase]).map(phase => ({ model, phase })));
        const built = [...crowns, ...own].map(({ model, phase }): BuiltCrown => {
            const season: Season = SEASONS[phase], grown = ([0, 1, 2] as Lod[]).map(lod => tree(model, season, lod));
            return { model, phase, ...arrays(bareIn(model, season) ? grown : tiers(grown)) };
        });
        self.postMessage({ id, leafy: species, crowns: built } satisfies ModelAnswer, { transfer: [...species, ...built].flatMap(m => m.tiers).flatMap(t => [t.position.buffer, t.normal.buffer, t.color.buffer, t.index.buffer]) });
    } catch (error) {
        self.postMessage({ id, error: error instanceof Error ? error.message : String(error) } satisfies ModelAnswer);
    }
};

/** Tier 0, and tiers 1 and 2 simplified from it, each with its own trunk. */
function tiers([near, middle, far]: Grown[]): Grown[] {
    const want = lit(near);
    return [near, simplified(near, middle, TRIANGLES[0], want), simplified(near, far, TRIANGLES[1], want)];
}

function arrays(grown: Grown[]): BuiltModel {
    return {
        tiers: grown.map(geo => ({
            position: Float32Array.from(geo.position), normal: Float32Array.from(geo.normal), color: Float32Array.from(geo.color), index: Uint32Array.from(geo.index), trunk: geo.trunk,
        })),
        crown: Math.min(...grown.map(geo => geo.crown)),
    };
}
