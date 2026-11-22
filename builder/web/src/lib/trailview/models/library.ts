// The model library: one file per model in trees/, holding all that the model is. The file's name is
// the model's id, by which packs name it.

import type { Season } from '../rules/regions';
import { grow, type Lod, type Tree } from './trees';

const byId = <T>(files: Record<string, unknown>) =>
    Object.fromEntries(Object.entries(files).map(([path, model]) => [path.split('/').pop()!.replace(/\.ts$/, ''), model as T]));

export const TREES = byId<Tree>(import.meta.glob('./trees/*.ts', { eager: true, import: 'default' }));

/** A tree model, 1 tall. */
export const tree = (id: string, season: Season, lod: Lod, seed = 1) => grow(TREES[id], season, lod, seed);

/** Whether a tree model stands bare in a season. */
export const bareIn = (id: string, season: Season) => {
    const spec = TREES[id];
    return (spec.form === 'conifer' ? spec.needles : spec.leaves)[season] === 'bare';
};

/**
 * Whether a species builds the far tier of its leafy crown: its thin limbs, sprays or fronds are too small to
 * simplify from the nearest tier, and would come apart.
 */
export const buildsFar = (id: string) => {
    const spec = TREES[id];
    return spec.form === 'conifer' ? !!(spec.boughs || spec.tufts) : (!!spec.crook && !spec.shape) || !!spec.fronds;
};

/** Whether a species' far tier is simplified from its own far level, not from its nearest: a shaped crown of many small lumps would come out in shards. */
export const farFromFar = (id: string) => {
    const spec = TREES[id];
    return spec.form === 'broadleaf' && !!spec.shape;
};

/** Whether a species grows every tier of its leafy crown: rosettes of cards, which simplify into shards. */
export const growsTiers = (id: string) => {
    const spec = TREES[id];
    return spec.form === 'broadleaf' && !!spec.branching;
};
