// Builds tree models off the main thread: three tiers of each, as plain arrays.
import type { Season } from '../rules/regions';
import { tree, TREES } from '../models/library';
import { simplified, simplifierReady } from '../models/simplify';
import type { Lod } from '../models/trees';

/**
 * The crown triangles of tiers 1 and 2, which the simplifier makes from tier 0. A bare crown's tiers
 * stay as the models build them: their twigs grow fewer and wider with the distance.
 */
const TRIANGLES = [1000, 300];

/** The models to build, each in a season. */
export interface ModelJob { id: number; models: { model: string; season: Season }[] }
export interface BuiltTier { position: Float32Array; normal: Float32Array; color: Float32Array; index: Uint32Array }
/** A model's tiers, each with its trunk's vertices first, and its crown's lowest point in model units at every tier. */
export interface BuiltModel { tiers: (BuiltTier & { trunk: number })[]; crown: number }

self.onmessage = async ({ data: { id, models } }: MessageEvent<ModelJob>) => {
    try {
        await simplifierReady;
        const built = models.map(({ model, season }): BuiltModel => {
            const spec = TREES[model], own = ([0, 1, 2] as Lod[]).map(lod => tree(model, season, lod));
            const bare = (spec.form === 'conifer' ? spec.needles : spec.leaves)[season] === 'bare';
            const grown = bare ? own : [own[0], simplified(own[0], own[1], TRIANGLES[0]), simplified(own[0], own[2], TRIANGLES[1])];
            return {
                tiers: grown.map(geo => ({
                    position: Float32Array.from(geo.position), normal: Float32Array.from(geo.normal), color: Float32Array.from(geo.color), index: Uint32Array.from(geo.index), trunk: geo.trunk,
                })),
                crown: Math.min(...grown.map(geo => geo.crown)),
            };
        });
        self.postMessage({ id, built }, { transfer: built.flatMap(m => m.tiers).flatMap(t => [t.position.buffer, t.normal.buffer, t.color.buffer, t.index.buffer]) });
    } catch (error) {
        self.postMessage({ id, error: error instanceof Error ? error.message : String(error) });
    }
};
