/** The state of a route on the device, from the library state that the device sync writes. */
import type { LibraryState, RouteRecord } from '../../lib/library/records';
import { lineKey } from '../../lib/library/summary';

export type DeviceState = 'waiting' | 'sent' | 'changed';

export const stateText: Record<DeviceState, string> = {
    waiting: 'Waits for the device',
    sent: 'On the device',
    changed: 'Changed since it was sent',
};

/** A route that waits to go to the device; else a route on the device, which changed when its line is not the sent line. */
export function deviceState(route: RouteRecord, waiting: LibraryState['waiting'] | undefined, sent: LibraryState['sent'] | undefined): DeviceState | undefined {
    if (waiting?.includes(route.id)) return 'waiting';
    const copy = sent?.[route.id];
    return copy && (copy.line === lineKey(route) ? 'sent' : 'changed');
}

/** Adds the route to the routes that wait for the device, or takes it out. */
export const waitingWith = (id: string, on: boolean) => (ids: string[] | undefined) =>
    on ? [...(ids ?? []).filter(other => other !== id), id] : (ids ?? []).filter(other => other !== id);
