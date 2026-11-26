/** Writes the trip object of a plan of several days: the list of its day routes on the device (`specs/ble-interface.md` §7.7). */
import { DISPLAY_NAME_MAX } from '../device/write';
import { truncateUtf8 } from '../format';
import { orderedRoutePoints, partDays, type Trip } from '../planner/editor';

/** One day of a trip object: the object id of its route, and the metres along the route where it joins and leaves the main
 * line. */
export interface TripDay {
    route: bigint;
    joinM: number;
    leaveM: number;
}

/** The trip object v3: a header of 64 bytes, then 16 bytes for each day, little-endian. The name is cut to 48 bytes on a
 * character boundary. `startDate` is in days since 1970-01-01, 0 for none. */
export function tripObject(key: bigint, name: string, startDate: number, days: TripDay[]): Uint8Array {
    const text = new TextEncoder().encode(truncateUtf8(name, DISPLAY_NAME_MAX));
    const bytes = new Uint8Array(64 + 16 * days.length), view = new DataView(bytes.buffer);
    bytes[0] = 3;
    view.setUint16(2, days.length, true);
    bytes[4] = text.length;
    bytes.set(text, 5);
    view.setUint16(54, startDate, true);
    view.setBigUint64(56, key, true);
    days.forEach((day, i) => {
        view.setBigUint64(64 + 16 * i, day.route, true);
        view.setUint32(72 + 16 * i, day.joinM, true);
        view.setUint32(76 + 16 * i, day.leaveM, true);
    });
    return bytes;
}

/** The trip key of part `part` of the plan `id`: FNV-1a 64 of "‹plan id›|‹id of route point 1›|‹id of route point 2›|‹part›",
 * never 0. It stays while the first two route points keep their ids, so the device keeps the progress of the trip. A
 * reversal or a new point right after the start makes a new key, and the progress starts empty. */
export function tripKey(id: string, trip: Trip, part: number): bigint {
    const [first, second] = orderedRoutePoints(trip);
    let hash = 0xcbf29ce484222325n;
    for (const byte of new TextEncoder().encode(`${id}|${first?.id}|${second?.id}|${part}`)) hash = (hash ^ BigInt(byte)) * 0x100000001b3n & 0xffffffffffffffffn;
    return hash || 1n;
}

/** The name of part `part` of `parts`: the plan name with " 1/2" or " 2/2" at the end, inside 48 bytes; the plan name is
 * cut, never the end. One part has the plan name. */
export function partName(name: string, part: number, parts: number): string {
    if (parts === 1) return name;
    const suffix = ` ${part}/${parts}`;
    return truncateUtf8(name, DISPLAY_NAME_MAX - suffix.length).trimEnd() + suffix;
}

/** The start date of part `part` of a trip in days since 1970-01-01, or 0 when the trip has none. */
export function startDay(trip: Trip, part: number): number {
    return trip.startDate ? Date.parse(trip.startDate) / 86_400_000 + partDays * (part - 1) : 0;
}
