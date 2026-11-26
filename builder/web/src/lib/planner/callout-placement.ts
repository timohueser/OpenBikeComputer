/** A MapLibre popup anchor: the side or corner of the popup that touches its point. */
export type Anchor = 'bottom' | 'top' | 'bottom-left' | 'bottom-right' | 'top-left' | 'top-right' | 'left' | 'right';
export interface Box { left: number; top: number; right: number; bottom: number }

// Above the point first, then below, then beside it.
const ANCHORS: Anchor[] = ['bottom', 'top', 'bottom-left', 'bottom-right', 'top-left', 'top-right', 'left', 'right'];
/** The tip of a MapLibre popup: 10 px deep. */
export const TIP = 10;

/** The box MapLibre draws for `anchor`, with a number `offset` as MapLibre spreads it over the anchors. Above and below
 * the point, the tip is `tip` px from the left edge of the popup, not at its middle. */
export function calloutBox(anchor: Anchor, [x, y]: [number, number], [width, height]: [number, number], offset: number, tip = width / 2): Box {
    const corner = Math.round(offset / Math.SQRT2);
    const [vertical, horizontal = ''] = anchor === 'left' || anchor === 'right' ? ['', anchor] : anchor.split('-');
    // The tip adds to the height above or below the point, and to the width beside it.
    const w = width + (vertical ? 0 : TIP), h = height + (vertical ? TIP : 0);
    const dx = horizontal === 'left' ? (vertical ? corner : offset) : horizontal === 'right' ? -(vertical ? corner : offset) : 0;
    const dy = vertical === 'top' ? (horizontal ? corner : offset) : vertical === 'bottom' ? -(horizontal ? corner : offset) : 0;
    const left = x + dx - (horizontal === 'left' ? 0 : horizontal === 'right' ? w : vertical ? tip : w / 2);
    const top = y + dy - (vertical === 'top' ? 0 : vertical === 'bottom' ? h : h / 2);
    return { left, top, right: left + w, bottom: top + h };
}

/** How far the map must pan to bring `box` inside `free`. */
function overflow(box: Box, free: Box): [number, number] {
    const axis = (low: number, high: number, min: number, max: number) => low < min ? low - min : Math.max(0, high - max);
    return [axis(box.left, box.right, free.left, free.right), axis(box.top, box.bottom, free.top, free.bottom)];
}

/**
 * The first anchor whose popup fits inside `free`, the map area clear of bars and controls.
 * When none fits, the anchor that needs the shortest pan, and that pan. A box larger than `free` fits after no pan, so
 * it is the last choice.
 */
export function placeCallout(point: [number, number], size: [number, number], free: Box, offset: number, tip?: number): { anchor: Anchor; pan: [number, number] } {
    let best: { anchor: Anchor; pan: [number, number]; larger: boolean } | undefined;
    for (const anchor of ANCHORS) {
        const box = calloutBox(anchor, point, size, offset, tip), pan = overflow(box, free);
        if (!pan[0] && !pan[1]) return { anchor, pan };
        const larger = box.right - box.left > free.right - free.left || box.bottom - box.top > free.bottom - free.top;
        if (!best || larger < best.larger || (larger === best.larger && Math.hypot(...pan) < Math.hypot(...best.pan))) best = { anchor, pan, larger };
    }
    return { anchor: best!.anchor, pan: best!.pan };
}
