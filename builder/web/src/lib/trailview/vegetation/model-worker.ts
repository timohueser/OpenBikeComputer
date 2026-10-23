// Builds tree models off the main thread: three tiers of each, as plain arrays.
import type { Season } from '../rules/regions';
import { tree } from '../models/library';
import type { Lod } from '../models/trees';

/** The models to build, each in a season. */
export interface ModelJob { id: number; models: { model: string; season: Season }[] }
export interface BuiltTier { position: Float32Array; normal: Float32Array; color: Float32Array; index: Uint32Array }

self.onmessage = ({ data: { id, models } }: MessageEvent<ModelJob>) => {
    try {
        const built = models.map(({ model, season }) => ([0, 1, 2] as Lod[]).map((lod): BuiltTier => {
            const geo = tree(model, season, lod);
            return { position: Float32Array.from(geo.position), normal: Float32Array.from(geo.normal), color: Float32Array.from(geo.color), index: Uint32Array.from(geo.index) };
        }));
        self.postMessage({ id, built }, { transfer: built.flat().flatMap(t => [t.position.buffer, t.normal.buffer, t.color.buffer, t.index.buffer]) });
    } catch (error) {
        self.postMessage({ id, error: error instanceof Error ? error.message : String(error) });
    }
};
