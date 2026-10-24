// Builds tree models off the main thread: three tiers of each, as plain arrays.
import { SEASONS } from '../rules/regions';
import { bareIn, tree } from '../models/library';
import { fits, tone } from '../models/tone';
import { simplified, simplifierReady } from '../models/simplify';
import type { Grown, Lod } from '../models/trees';
import { SUMMER } from './phenology';

/**
 * The crown triangles of tiers 1 and 2, which the simplifier makes from tier 0. A bare crown's tiers
 * stay as the models build them: their twigs grow fewer and wider with the distance.
 */
const TRIANGLES = [600, 200];
/** A phase draws the summer crown through a colour matrix where that turns every crown vertex within this many sRGB steps of 255. */
const FIT = 8;

/**
 * The models to build: a species, kept green all year or not, and the phases (phenology.ts) that its
 * trees have on the date.
 */
export interface ModelJob { id: number; models: { model: string; evergreen: boolean; phases: number[] }[] }
export interface BuiltTier { position: Float32Array; normal: Float32Array; color: Float32Array; index: Uint32Array }
/** A model's tiers, each with its trunk's vertices first, and its crown's lowest point in model units at every tier. */
export interface BuiltModel { tiers: (BuiltTier & { trunk: number })[]; crown: number }
/**
 * A species: its leafy crown in summer, with per phase (winter first) the colour matrix (3 × 3, columns
 * first) that turns its colours into that phase's, of the least error; and per phase that the date
 * has, a crown of its own where the matrix does not turn the summer crown within FIT: a bare crown, or
 * flecks that the season moves. A crown of its own that differs only in colour draws the summer
 * crown's impostor through the matrix: far away its flecks merge.
 */
export interface BuiltSpecies { leafy: BuiltModel & { tones: (number[] | undefined)[] }; own: (BuiltModel | undefined)[] }

self.onmessage = async ({ data: { id, models } }: MessageEvent<ModelJob>) => {
    try {
        await simplifierReady;
        const built = models.map(({ model, evergreen, phases }): BuiltSpecies => {
            const summer = ([0, 1, 2] as Lod[]).map(lod => tree(model, 'summer', lod));
            const leafy = [summer[0], simplified(summer[0], summer[1], TRIANGLES[0]), simplified(summer[0], summer[2], TRIANGLES[1])];
            const tones: (number[] | undefined)[] = [], own: (BuiltModel | undefined)[] = [];
            SEASONS.forEach((season, phase) => {
                if (evergreen || phase === SUMMER) return;
                const near = bareIn(model, season) ? undefined : tree(model, season, 0), matrix = near && tone(summer[0], near);
                tones[phase] = matrix;
                if ((!matrix || !fits(matrix, summer[0], near!, FIT)) && phases.includes(phase)) own[phase] = arrays(near ? [near, simplified(near, tree(model, season, 1), TRIANGLES[0]), simplified(near, tree(model, season, 2), TRIANGLES[1])] : ([0, 1, 2] as Lod[]).map(lod => tree(model, season, lod)));
            });
            return { leafy: { ...arrays(leafy), tones }, own };
        });
        const all = built.flatMap(({ leafy, own }) => [leafy, ...own.filter(crown => crown !== undefined)]);
        self.postMessage({ id, built }, { transfer: all.flatMap(m => m.tiers).flatMap(t => [t.position.buffer, t.normal.buffer, t.color.buffer, t.index.buffer]) });
    } catch (error) {
        self.postMessage({ id, error: error instanceof Error ? error.message : String(error) });
    }
};

function arrays(grown: Grown[]): BuiltModel {
    return {
        tiers: grown.map(geo => ({
            position: Float32Array.from(geo.position), normal: Float32Array.from(geo.normal), color: Float32Array.from(geo.color), index: Uint32Array.from(geo.index), trunk: geo.trunk,
        })),
        crown: Math.min(...grown.map(geo => geo.crown)),
    };
}
