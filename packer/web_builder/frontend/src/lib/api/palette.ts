import { api } from "./client";

export interface Palette {
    columns: number;
    colors: string[];
}

let cached: Promise<Palette> | null = null;

/** The device's 64-color gamut for the picker grid (fetched once, shared). */
export function getPalette(): Promise<Palette> {
    cached ??= api
        .palette()
        .then((p) => ({ columns: p.columns > 0 ? p.columns : 8, colors: p.colors ?? [] }))
        .catch(() => ({ columns: 8, colors: [] })); // popover falls back to the OS picker
    return cached;
}
