// Builds the plant models off the main thread, as plain arrays: a plant in one of its shapes.
import type { Season } from '../rules/regions';
import { plant } from '../models/plant-library';

/** The models to build, each in a season: a plant's shape by its seed. */
export interface PlantJob { id: number; models: { model: string; seed: number; season: Season }[] }
export interface BuiltMesh { position: Float32Array; normal: Float32Array; color: Float32Array; index: Uint32Array }

self.onmessage = ({ data: { id, models } }: MessageEvent<PlantJob>) => {
    try {
        const built = models.map(({ model, seed, season }): BuiltMesh => {
            const geo = plant(model, season, seed);
            return { position: Float32Array.from(geo.position), normal: Float32Array.from(geo.normal), color: Float32Array.from(geo.color), index: Uint32Array.from(geo.index) };
        });
        self.postMessage({ id, built }, { transfer: built.flatMap(m => [m.position.buffer, m.normal.buffer, m.color.buffer, m.index.buffer]) });
    } catch (error) {
        self.postMessage({ id, error: error instanceof Error ? error.message : String(error) });
    }
};
