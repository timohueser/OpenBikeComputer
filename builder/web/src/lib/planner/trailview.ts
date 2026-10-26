/** The heading of a new view, in degrees clockwise from north. */
export const FIRST_HEADING = 0;
/** The binoculars hang this far below the pointer, in CSS pixels. Their bottom is the drop point. */
export const HANG_PX = 36;
const DEGREES_PER_PX = 1.5, MAX_SWING = 35, SETTLE_MS = 140;

/** A press that moves farther than this, in CSS pixels, is a drag; a shorter one is a click. A finger wobbles more than a mouse. */
export function dragPx(pointerType: string): number {
    return pointerType === 'touch' ? 10 : 5;
}

/** The engine is a chunk of its own: the planner loads it at the first press of the binoculars. */
let chunk: Promise<typeof import('./trailview-start')> | undefined;
export function loadTrailView(): Promise<typeof import('./trailview-start')> {
    return chunk ??= import('./trailview-start').catch(error => {
        chunk = undefined;
        throw error;
    });
}

/** The swing after the pointer moves `dx` pixels: the binoculars lag behind, so a move to the right turns them clockwise. */
export function swing(angle: number, dx: number): number {
    return Math.max(-MAX_SWING, Math.min(MAX_SWING, angle + dx * DEGREES_PER_PX));
}

/** The swing `ms` later, as it eases back to rest. */
export function settle(angle: number, ms: number): number {
    return angle * Math.exp(-ms / SETTLE_MS);
}

/** The drop point of the binoculars that hang from the pointer at `angle` degrees clockwise: their bottom. */
export function dropPoint([x, y]: [number, number], angle: number): [number, number] {
    const radians = angle * Math.PI / 180;
    return [x - HANG_PX * Math.sin(radians), y + HANG_PX * Math.cos(radians)];
}
