/**
 * Route retention, in words: the six levels §4.4 cmd 6 takes and the route list entry reports,
 * and the one-phrase reading of a route's expiry clock the tiles show.
 *
 * The wire is a plain byte (`0` never … `5` two months) and stays one — this module exists so
 * the drop tile's picker, the ⋯ menu and the expiry tag all say the same words for the same
 * level, and so the phrase logic is testable without a component around it.
 */

import type { RouteListEntry } from "../usb/objects";

/** The wire values of §4.4 cmd 6, in menu order. `0` is "forever" and the upload default. */
export const RETENTION_LEVELS = [0, 1, 2, 3, 4, 5] as const;

export type RetentionLevel = (typeof RETENTION_LEVELS)[number];

const LABELS: Record<RetentionLevel, string> = {
    0: "forever",
    1: "1 day",
    2: "1 week",
    3: "2 weeks",
    4: "1 month",
    5: "2 months",
};

/** The picker's word for a wire level. Unknown levels (newer firmware) are named, not hidden. */
export function retentionLabel(level: number): string {
    return LABELS[level as RetentionLevel] ?? `level ${level}`;
}

/**
 * What the retention clock means for this route, in one short phrase — the tile's tag.
 *
 * `expiresAt === 0` with a non-zero retention is a real state, not a decoding gap: the device
 * anchors expiry to last *use* and has no RTC, so a route uploaded before any peer set the
 * trusted clock has a level but no started countdown.
 */
export function expiryPhrase(
    route: Pick<RouteListEntry, "retention" | "expiresAt">,
    now: number = Date.now(),
): string {
    if (route.retention === 0) return "kept forever";
    if (route.expiresAt === 0) return "expiry not started";
    const days = Math.ceil((route.expiresAt * 1000 - now) / 86_400_000);
    if (days <= 0) return "expiring";
    return days === 1 ? "expires tomorrow" : `expires in ${days} days`;
}

/** True where {@link expiryPhrase}'s tag deserves the warning color: a running (or overrun)
 *  countdown. A level whose clock has not started threatens nothing yet. */
export function expiryWarns(route: Pick<RouteListEntry, "retention" | "expiresAt">): boolean {
    return route.retention !== 0 && route.expiresAt !== 0;
}
