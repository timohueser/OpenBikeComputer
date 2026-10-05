/** Elevation drawer heights: the smallest open one, and the closed one with its title row only. */
export const DRAWER_MIN = 210, DRAWER_CLOSED = 44;

/**
 * The drawer height after a separator move from `shown` to `value`; DRAWER_CLOSED is closed.
 * A key step crosses the gap between closed and the open minimum in its direction. A drag closes
 * past halfway and holds at the open minimum before it.
 */
export function resizeDrawer(shown: number, value: number, byKey: boolean): number {
    if (value >= DRAWER_MIN) return value;
    if (byKey) return value > shown || (shown > DRAWER_MIN && value > DRAWER_CLOSED) ? DRAWER_MIN : DRAWER_CLOSED;
    return value >= (DRAWER_MIN + DRAWER_CLOSED) / 2 ? DRAWER_MIN : DRAWER_CLOSED;
}
