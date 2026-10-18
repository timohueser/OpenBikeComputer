/**
 * The model library: one file per model in trees/, plants/ and animals/, holding all that model is.
 * The file's name is the model's id, by which packs name it.
 */

import type * as THREE from 'three';
import { unit } from './shape';
import { grow, type Lod, type Season, type Tree } from './trees';

/** WorldCover classes a ground plant stands on. */
export const COVERS = ['tree', 'shrub', 'grass', 'crop', 'bare', 'wetland', 'moss'] as const;
export type Cover = (typeof COVERS)[number];

/** Where a ground plant grows, how tall it stands, and the share of grid points it takes there. */
export interface Ground {
  on: Cover[];
  /** In metres; a plant model standing as a shrub takes it too. */
  height: [number, number];
  share: number;
  /**
   * In a region with moor, a moor plant also grows on grass that the satellite sees as moor, and a
   * lush one only off it: the shader decides per plant from the same ground the terrain paints.
   */
  habitat?: 'moor' | 'lush';
  /** Its own colour is its character, so it keeps it rather than taking the ground's. */
  own?: true;
  /** It grows in stands: patches a few tens of metres across. */
  clumped?: true;
}

export interface Plant {
  name: string;
  latin: string;
  /** The plant at any size with its root at the origin and z up; the seed picks its shape. */
  build: (season: Season, seed: number) => THREE.BufferGeometry;
  ground: Ground;
}

export interface Animal {
  name: string;
  latin: string;
  /** The animal at any size with its feet at the origin and z up; the seed picks the coat where a breed has several. */
  build: (seed: number) => THREE.BufferGeometry;
  height_m: number;
}

const byId = <T>(files: Record<string, unknown>) =>
  Object.fromEntries(Object.entries(files).map(([path, model]) => [path.split('/').pop()!.replace(/\.ts$/, ''), model as T]));

export const TREES = byId<Tree>(import.meta.glob('./trees/*.ts', { eager: true, import: 'default' }));
export const PLANTS = byId<Plant>(import.meta.glob('./plants/*.ts', { eager: true, import: 'default' }));
export const ANIMALS = byId<Animal>(import.meta.glob('./animals/*.ts', { eager: true, import: 'default' }));

// A pack names a model by its id alone.
const kinds = new Map<string, string>();
for (const [kind, models] of Object.entries({ tree: TREES, plant: PLANTS, animal: ANIMALS })) {
  for (const id of Object.keys(models)) {
    if (kinds.has(id)) throw new Error(`model ${id} is both a ${kinds.get(id)} and a ${kind}`);
    kinds.set(id, kind);
  }
}

/** Models are 1 tall: the instanced shader scales each by its own height. */
export const tree = (id: string, season: Season, lod: Lod, seed = 1) => grow(TREES[id], season, lod, seed);
export const plant = (id: string, season: Season, seed = 1) => unit(PLANTS[id].build(season, seed));
export const animal = (id: string, seed = 0) => unit(ANIMALS[id].build(seed));
