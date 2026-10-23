// The kind registry: each module of kinds/ but this file and kind.ts draws the features of the kind of
// its name. The models that kinds stand on the surface are numbered in the registry's order, the same in
// the stamp worker and on the main thread.
import type { Kind } from './kind';

const MODULES = import.meta.glob<Kind>(['./*.ts', '!./index.ts', '!./kind.ts', '!./*.test.ts'], { eager: true, import: 'default' });

export const KINDS = new Map(Object.entries(MODULES).map(([path, kind]) => [path.slice(2, -3), kind]));

/** The models of the kinds, as `kind/model`, in the order of their numbers. */
export const POINT_MODELS = [...KINDS].flatMap(([name, kind]) => Object.keys(kind.models ?? {}).map(model => `${name}/${model}`));
