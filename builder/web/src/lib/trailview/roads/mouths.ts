// The mouths of culverts as models on the surface. `standMouths` of terrain/culverts.ts places them.
import { culvertMouth } from '../models/amenities';
import { unitModel, type Build, type Kind } from '../kinds/kind';
import type { Shapes } from '../terrain/bins';

/** Mouths draw as far as the corridors do. */
const MOUTH_REACH_M = 600;
const mouth = unitModel(culvertMouth, MOUTH_REACH_M);

/** The kind of streams. A mouth casts no shadow: at this size the shadow map draws a dark, detached blob. */
export const mouthKind: Kind = { build() {}, models: { mouth: () => ({ ...mouth.spec(), casts: false }) } };

/** Stands the mouths of a tile that a node owns: the model's front, its −y, toward (ux, uy) of world axes, whose y runs south. */
export function culvertMouths(out: Pick<Build, 'owns' | 'place'>, { mouths }: Pick<Shapes, 'mouths'>): void {
    for (const { x, y, ux, uy } of mouths) if (out.owns(x, y)) out.place({ x, y, yaw: Math.atan2(ux, uy), height: mouth.height, model: 'mouth' });
}
