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
