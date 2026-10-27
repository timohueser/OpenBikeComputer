// The ground plants: one file per plant in plants/, holding all that the plant is. The file's name is
// the plant's id, by which packs name it.

import type { Season } from '../rules/regions';
import { unit, type Geometry } from './shape';

/**
 * Where a ground plant grows, how tall it stands, and the share of grid points that it takes there.
 * `on` and `off` name ground as covers, materials or `#tag`s of materials.json: it grows on what `on`
 * names but `off` does not.
 */
export interface Ground {
    on: string[];
    off?: string[];
    /** In metres; a plant model that stands as a shrub takes it too. */
    height: [number, number];
    share: number;
    /** It keeps its own colour rather than the ground's. */
    own?: true;
    /** It grows in stands: patches a few tens of metres across. */
    clumped?: boolean;
}

export interface Plant {
    name: string;
    latin: string;
    /** The plant at any size with its root at the origin and z up; the seed picks its shape. */
    build: (season: Season, seed: number) => Geometry;
    ground: Ground;
    /** It builds itself 1 tall in its tallest season and keeps that scale, so `height` stays metres and stubble stands lower than the crop. */
    sized?: true;
}

const byId = <T>(files: Record<string, unknown>) =>
    Object.fromEntries(Object.entries(files).map(([path, model]) => [path.split('/').pop()!.replace(/\.ts$/, ''), model as T]));

/** The files of plants/ that are plants; a file may also export a builder that another plant reuses. */
export const PLANTS = byId<Plant>(import.meta.glob('./plants/*.ts', { eager: true, import: 'default' }));

/** A plant model, 1 tall, or in its own scale if it is sized. */
export const plant = (id: string, season: Season, seed = 1) => (PLANTS[id].sized ? PLANTS[id].build(season, seed) : unit(PLANTS[id].build(season, seed)));
