// Builds tree models off the main thread: three tiers of each, as plain arrays.
import type { Season } from '../rules/regions';
import { tree } from '../models/library';
import type { Lod } from '../models/trees';

/** The models to build, each in a season. */
export interface ModelJob { id: number; models: { model: string; season: Season }[] }
export interface BuiltTier { position: Float32Array; normal: Float32Array; color: Float32Array; index: Uint32Array }
/** A model's tiers, each with its trunk's vertices first, and its crown's lowest point in model units at every tier. */
export interface BuiltModel { tiers: (BuiltTier & { trunk: number })[]; crown: number }

self.onmessage = ({ data: { id, models } }: MessageEvent<ModelJob>) => {
    try {
        const built = models.map(({ model, season }): BuiltModel => {
            const grown = ([0, 1, 2] as Lod[]).map(lod => tree(model, season, lod));
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
