// Builds the ground plants and the shrubs off the main thread, as plain arrays: a plant model at one
// level of detail, a tree model that stands as a shrub at its two coarser levels.
import type { Season } from '../rules/regions';
import { tree } from '../models/library';
import { plant } from '../models/plant-library';
import type { Geometry } from '../models/shape';

/** A model to build: a plant, or a tree in its tiers `lods`, in a season. */
export interface PlantJob { id: number; models: { model: string; tree: boolean; season: Season }[] }
export interface BuiltMesh { position: Float32Array; normal: Float32Array; color: Float32Array; index: Uint32Array }

const arrays = (geo: Geometry): BuiltMesh => ({
    position: Float32Array.from(geo.position), normal: Float32Array.from(geo.normal), color: Float32Array.from(geo.color), index: Uint32Array.from(geo.index),
});

self.onmessage = ({ data: { id, models } }: MessageEvent<PlantJob>) => {
    try {
        const built = models.map(({ model, tree: isTree, season }) => (isTree ? [tree(model, season, 1), tree(model, season, 2)] : [plant(model, season, 2)]).map(arrays));
        self.postMessage({ id, built }, { transfer: built.flat().flatMap(m => [m.position.buffer, m.normal.buffer, m.color.buffer, m.index.buffer]) });
    } catch (error) {
        self.postMessage({ id, error: error instanceof Error ? error.message : String(error) });
    }
};
