// The ground plants: one file per plant in plants/, holding all that the plant is. The file's name is
// the plant's id, by which packs name it.

import type { Season } from '../rules/regions';
import { unit, type Geometry } from './shape';

/** The covers of the ground that a plant stands on. */
export const COVERS = ['tree', 'shrub', 'grass', 'crop', 'bare', 'wetland', 'moss'] as const;
export type Cover = (typeof COVERS)[number];

/** Where a ground plant grows, how tall it stands, and the share of grid points that it takes there. */
export interface Ground {
    on: Cover[];
    /** In metres; a plant model that stands as a shrub takes it too. */
    height: [number, number];
    share: number;
    /** In a region with moor, a moor plant grows on its heath and bog, and a lush one only off them. */
    habitat?: 'moor' | 'lush';
    /** It keeps its own colour rather than the ground's. */
    own?: true;
    /** It grows in stands: patches a few tens of metres across. */
    clumped?: true;
}

export interface Plant {
    name: string;
    latin: string;
    /** The plant at any size with its root at the origin and z up; the seed picks its shape. */
    build: (season: Season, seed: number) => Geometry;
    ground: Ground;
}

const byId = <T>(files: Record<string, unknown>) =>
    Object.fromEntries(Object.entries(files).map(([path, model]) => [path.split('/').pop()!.replace(/\.ts$/, ''), model as T]));

/** The files of plants/ that are plants; heather.ts also exports its builder for heather-bloom.ts. */
export const PLANTS = byId<Plant>(import.meta.glob('./plants/*.ts', { eager: true, import: 'default' }));

/** A plant model, 1 tall. */
export const plant = (id: string, season: Season, seed = 1) => unit(PLANTS[id].build(season, seed));
