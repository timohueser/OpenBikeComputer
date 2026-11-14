/** Remembers the one device that the sync works with, and what the last sync found on it, in `localStorage`. */
import { deviceHolder } from '../device/session.svelte';
import type { MapBox } from '../library/records';

/** Bytes on the card by the kind of object. */
export interface DeviceBytes { map: number; routes: number; rides: number; other: number }

export interface RememberedDevice {
    serial: string;
    /** The card of the device. Another card makes `skipped` void. */
    storeId: string;
    /** Unix milliseconds of the end of the last sync, or null before the first. */
    at: number | null;
    /** The name of the map on the card, or null without one. */
    map: string | null;
    /** `objectId:revision` of the map on the card, or null without one. */
    mapObject: string | null;
    /** The area and the colours of the map `object` that this browser sent to the card. */
    built: { object: string; regions: string[]; boxes: MapBox[]; light: string; dark: string } | null;
    routes: number;
    bytes: DeviceBytes;
    /** The rides that came in on `day`, a local `YYYY-MM-DD`. */
    came: { day: string; count: number } | null;
    /** The `ObjectId`s of the rides that the library does not take: demo rides and rides with no points or no clock. */
    skipped: string[];
}

const KEY = 'obc-device';

function isDevice(value: unknown): value is RememberedDevice {
    const v = value as Partial<RememberedDevice> | null;
    return !!v && typeof v.serial === 'string' && typeof v.storeId === 'string' && Array.isArray(v.skipped) && typeof v.bytes === 'object';
}

function stored(): RememberedDevice | null {
    try {
        const value = JSON.parse(localStorage.getItem(KEY) ?? 'null');
        return isDevice(value) ? value : null;
    } catch {
        return null;
    }
}

/** The remembered device. Before a Connect button remembers one, it is null and no sync runs. */
export const remembered = $state<{ device: RememberedDevice | null }>({ device: stored() });

// Another tab remembered a device or synced.
if (typeof window !== 'undefined') {
    window.addEventListener('storage', event => { if (event.key === KEY || event.key === null) remembered.device = stored(); });
}

function save(device: RememberedDevice | null): void {
    remembered.device = device;
    try { localStorage.setItem(KEY, JSON.stringify(device)); } catch { /* The device stays remembered for this page. */ }
}

/** Merges `change` onto the remembered device; nothing without one. */
export function remember(change: Partial<RememberedDevice>): void {
    const device = stored() ?? $state.snapshot(remembered.device);
    if (device) save({ ...device, ...change });
}

const listeners = new Set<() => void>();

/** Calls `listener` after each `rememberDevice`. Returns the function that stops it. */
export function onRemember(listener: () => void): () => void {
    listeners.add(listener);
    return () => listeners.delete(listener);
}

/** Remembers the card `storeId` of the device `serial`, with a new record for a new serial or card. */
export function rememberCard(serial: string, storeId: string): void {
    const device = remembered.device;
    if (device?.serial === serial && device.storeId === storeId) return;
    save({ serial, storeId, at: null, map: null, mapObject: null, built: null, routes: 0, bytes: { map: 0, routes: 0, rides: 0, other: 0 }, came: null, skipped: [] });
}

/** Remembers the connected device. Only the Connect button of the Device page calls it; a sync follows. */
export function rememberDevice(info: { serialNumber: string }, store: { storeId: string }): void {
    rememberCard(info.serialNumber, store.storeId);
    for (const listener of listeners) listener();
}

/** The factory name of a device: `OBC-` and the last four hex digits of its serial, as the device names itself. */
export const deviceName = (serial: string) => `OBC-${serial.slice(-4).toUpperCase()}`;

/** Whether the remembered device is the `ready` device of the session, with a card that has a format. Reactive where a
 * component reads it. */
export function rememberedReady(): boolean {
    const session = deviceHolder.session;
    return session?.status === 'ready' && !!session.store && !!remembered.device && session.info?.serialNumber === remembered.device.serial;
}
