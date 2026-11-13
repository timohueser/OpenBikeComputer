export type Units = 'km' | 'mi';

/** A value of the rider. `at` is in Unix seconds: the start of the ride that brought it, or the time of the edit. */
export interface RiderValue {
    value: number;
    at: number;
    device: boolean;
}

/** The look is not here: it stays in the key `obc-theme`, which the project site shares (`lib/theme.svelte.ts`). */
export interface Settings {
    units: Units;
    maxHr: RiderValue | null;
    ftp: RiderValue | null;
}

const KEY = 'obc-settings';
/** The ranges of the device (`firmware/obc-app/src/effort.rs`). */
export const RANGES = { maxHr: { min: 120, max: 220 }, ftp: { min: 50, max: 600 } } as const;
const MAX_HR = [RANGES.maxHr.min, RANGES.maxHr.max] as const;
const FTP = [RANGES.ftp.min, RANGES.ftp.max] as const;

function riderValue(value: unknown, [low, high]: readonly [number, number]): RiderValue | null {
    if (typeof value !== 'object' || value === null) return null;
    const { value: number, at, device } = value as Record<string, unknown>;
    if (typeof number !== 'number' || !Number.isFinite(number) || typeof at !== 'number' || !Number.isFinite(at) || typeof device !== 'boolean') return null;
    return { value: Math.min(Math.max(number, low), high), at, device };
}

/** The valid fields of `value`, and the defaults for the others. */
export function readSettings(value: unknown): Settings {
    const fields = typeof value === 'object' && value !== null ? value as Record<string, unknown> : {};
    return {
        units: fields.units === 'mi' ? 'mi' : 'km',
        maxHr: riderValue(fields.maxHr, MAX_HR),
        ftp: riderValue(fields.ftp, FTP),
    };
}

/** The stored settings, or null when the browser has no storage. */
function stored(): Settings | null {
    try {
        return readSettings(JSON.parse(localStorage.getItem(KEY) ?? 'null'));
    } catch {
        return null;
    }
}

export const settings: Settings = $state(stored() ?? readSettings(null));

// Another tab wrote the settings.
if (typeof window !== 'undefined') {
    window.addEventListener('storage', event => {
        if (event.key === KEY || event.key === null) Object.assign(settings, stored() ?? readSettings(null));
    });
}

/** Merges onto the stored settings, so that a change does not undo a newer write of another tab. */
export function updateSettings(change: Partial<Settings>): void {
    Object.assign(settings, readSettings({ ...(stored() ?? $state.snapshot(settings)), ...change }));
    try { localStorage.setItem(KEY, JSON.stringify(settings)); } catch { /* The settings still apply to this page without storage. */ }
}

/** The values that a ride from the device brings. They replace a missing value, or one older than the ride. */
export function takeDeviceValues(start: number, maxHr: number | null, ftp: number | null): void {
    const newer = (current: RiderValue | null, value: number | null) =>
        value !== null && (current === null || current.at < start) ? { value, at: start, device: true } : current;
    const current = stored() ?? settings;
    updateSettings({ maxHr: newer(current.maxHr, maxHr), ftp: newer(current.ftp, ftp) });
}
