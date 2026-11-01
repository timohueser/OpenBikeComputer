// The mouths of culverts: a model on the surface where a stream's ribbon stops at a way, on each side, and at each end
// of a culvert that buries a stream.
import { culvertMouth } from '../models/amenities';
import { unitModel, type Build, type Kind, type Placement } from '../kinds/kind';
import { cornerAt } from '../terrain/corners';
import { GAP_REACH_M, GIVE_WAY_M, wayGaps } from '../terrain/culverts';
import { metresPerUnit, STREAM, type Shapes } from '../terrain/bins';

/** Mouths draw as far as the corridors do. */
const MOUTH_REACH_M = 600;
const mouth = unitModel(culvertMouth, MOUTH_REACH_M);
/** A ribbon's stop is found to this many metres along its line. */
const STEP_M = 0.25;
/** The longest step near a way: a corner of a junction reaches past the way's edge. */
const NEAR_STEP_M = 2;
/** Mouths stand at least this far apart: the wall of a mouth is 2.5 m wide. */
const APART_M = 2.5;

/** The kind of streams. `culvertMouths` places their mouths, from the lines of the tile. */
export const mouthKind: Kind = { build() {}, models: { mouth: mouth.spec } };

/** The mouths of each tile, made once. */
const planned = new WeakMap<Shapes['lines'], Placement[]>();

/** The mouths of a tile's streams. */
export function mouthsOf({ lines, culvertEnds }: Pick<Shapes, 'lines' | 'culvertEnds'>): Placement[] {
    const gapAt = wayGaps(lines), out: Placement[] = [];
    /** A mouth at a world point, its front toward (ux, uy) in world axes, whose y runs south. */
    const add = (x: number, y: number, ux: number, uy: number) => {
        const unit = metresPerUnit(y);
        if (!out.some(m => Math.hypot(m.x - x, m.y - y) * unit < APART_M)) out.push({ x, y, yaw: Math.atan2(ux, uy), height: mouth.height, model: 'mouth' });
    };
    for (const { x, y, ux, uy } of culvertEnds) add(x, y, ux, uy);
    for (const line of lines) {
        if (line.kind !== STREAM) continue;
        let was: boolean | undefined, from: [number, number] | undefined;
        for (let v = 0; v + 1 < line.z.length; v++) {
            const [ax, ay, bx, by] = line.xy.slice(2 * v, 2 * v + 4), unit = metresPerUnit(ay), l = Math.hypot(bx - ax, by - ay) * unit;
            for (let s = 0; s <= l; ) {
                const x = ax + ((bx - ax) * s) / (l || 1), y = ay + ((by - ay) * s) / (l || 1), gap = gapAt(x, y);
                const under = (!!gap && gap.metres < GIVE_WAY_M) || !!cornerAt(lines, x, y);
                if (was !== undefined && under !== was) {
                    // The mouth stands at the last point of open water, its front away from the way.
                    const [mx, my] = under ? from! : [x, y], foot = gapAt(mx, my) ?? gap;
                    if (foot) {
                        const dx = mx - foot.x, dy = my - foot.y, d = Math.hypot(dx, dy) || 1;
                        add(mx, my, dx / d, dy / d);
                    }
                }
                was = under;
                from = [x, y];
                // The nearest way can come one metre nearer per metre along, so this stops short of its ribbon's stop.
                s += gap ? Math.max(STEP_M, Math.min(gap.metres - GIVE_WAY_M - STEP_M, NEAR_STEP_M)) : GAP_REACH_M;
            }
        }
    }
    return out;
}

/** Stands the mouths of a tile that a node owns. */
export function culvertMouths(out: Pick<Build, 'owns' | 'place'>, shapes: Pick<Shapes, 'lines' | 'culvertEnds'>): void {
    const mouths = planned.get(shapes.lines) ?? planned.set(shapes.lines, mouthsOf(shapes)).get(shapes.lines)!;
    for (const m of mouths) if (out.owns(m.x, m.y)) out.place(m);
}
