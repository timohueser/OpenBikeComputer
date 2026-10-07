/** A MapLibre popup anchor: the side or corner of the popup that touches its point. */
export type Anchor = 'bottom' | 'top' | 'bottom-left' | 'bottom-right' | 'top-left' | 'top-right' | 'left' | 'right';
export interface Box { left: number; top: number; right: number; bottom: number }

// Above the point first, then below, then beside it.
const ANCHORS: Anchor[] = ['bottom', 'top', 'bottom-left', 'bottom-right', 'top-left', 'top-right', 'left', 'right'];
/** The tip of a MapLibre popup: 10 px deep. */
const TIP = 10;

/** The box MapLibre draws for `anchor`, with a number `offset` as MapLibre spreads it over the anchors. */
export function calloutBox(anchor: Anchor, [x, y]: [number, number], [width, height]: [number, number], offset: number): Box {
    const corner = Math.round(offset / Math.SQRT2);
    const [vertical, horizontal = ''] = anchor === 'left' || anchor === 'right' ? ['', anchor] : anchor.split('-');
    // The tip adds to the height above or below the point, and to the width beside it.
    const w = width + (vertical ? 0 : TIP), h = height + (vertical ? TIP : 0);
    const dx = horizontal === 'left' ? (vertical ? corner : offset) : horizontal === 'right' ? -(vertical ? corner : offset) : 0;
    const dy = vertical === 'top' ? (horizontal ? corner : offset) : vertical === 'bottom' ? -(horizontal ? corner : offset) : 0;
    const left = x + dx - (horizontal === 'left' ? 0 : horizontal === 'right' ? w : w / 2);
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
 * When none fits, the anchor that needs the shortest pan, and that pan.
 */
export function placeCallout(point: [number, number], size: [number, number], free: Box, offset: number): { anchor: Anchor; pan: [number, number] } {
    let best: { anchor: Anchor; pan: [number, number] } | undefined;
    for (const anchor of ANCHORS) {
        const pan = overflow(calloutBox(anchor, point, size, offset), free);
        if (!pan[0] && !pan[1]) return { anchor, pan };
        if (!best || Math.hypot(...pan) < Math.hypot(...best.pan)) best = { anchor, pan };
    }
    return best!;
}
