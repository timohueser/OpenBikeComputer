export type Units = 'km' | 'mi';
export type Pace = 'easy' | 'moderate' | 'sporty' | 'fast';

/** A value of the rider. `at` is in Unix seconds: the start of the ride that brought it, or the time of the edit. */
export interface RiderValue {
    value: number;
    at: number;
    device: boolean;
}

/** The look is not here: it stays in the key `obc-theme`, which the project site shares (`lib/theme.svelte.ts`). */
export interface Settings {
    units: Units;
    pace: Pace;
    maxHr: RiderValue | null;
    ftp: RiderValue | null;
}

export const PACES: Record<Pace, { label: string; kmh: number }> = {
    easy: { label: 'Easy', kmh: 15 },
    moderate: { label: 'Moderate', kmh: 19 },
    sporty: { label: 'Sporty', kmh: 23 },
    fast: { label: 'Fast', kmh: 27 },
};

const KEY = 'obc-settings';
// The ranges of the device (`firmware/obc-app/src/effort.rs`).
const MAX_HR = [120, 220] as const;
const FTP = [50, 600] as const;

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
        pace: typeof fields.pace === 'string' && Object.hasOwn(PACES, fields.pace) ? fields.pace as Pace : 'moderate',
        maxHr: riderValue(fields.maxHr, MAX_HR),
        ftp: riderValue(fields.ftp, FTP),
    };
}

function stored(): Settings {
    try {
        return readSettings(JSON.parse(localStorage.getItem(KEY) ?? 'null'));
    } catch {
        return readSettings(null);
    }
}

export const settings: Settings = $state(stored());

export function updateSettings(change: Partial<Settings>): void {
    Object.assign(settings, readSettings({ ...$state.snapshot(settings), ...change }));
    try { localStorage.setItem(KEY, JSON.stringify(settings)); } catch { /* The settings still apply to this page without storage. */ }
}

/** The values that a ride from the device brings. They replace a missing value, or one older than the ride. */
export function takeDeviceValues(start: number, maxHr: number | null, ftp: number | null): void {
    const newer = (current: RiderValue | null, value: number | null) =>
        value !== null && (current === null || current.at < start) ? { value, at: start, device: true } : current;
    updateSettings({ maxHr: newer(settings.maxHr, maxHr), ftp: newer(settings.ftp, ftp) });
}
