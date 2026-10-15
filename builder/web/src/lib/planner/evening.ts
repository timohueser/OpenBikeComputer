import type { Coordinate } from './geo';
import { dataLayers } from './layers/registry';
import { solarPosition, solarTerms } from './layers/sun';

// The upper limb at the horizon, with refraction.
const horizon = -.833 * Math.PI / 180;

/** The instant of sunset at `coordinate` on `date` (YYYY-MM-DD), or null when the sun does not set or rise. */
export function sunsetAt(coordinate: Coordinate, date: string): number | null {
    const altitude = (instant: number) => solarPosition(coordinate, solarTerms(instant)).altitude;
    let low = Date.parse(`${date}T12:00:00Z`) - coordinate[0] / 15 * 3600000, high = low + 12 * 3600000;
    if (altitude(low) < horizon || altitude(high) > horizon) return null;
    for (let i = 0; i < 24; i++) {
        const middle = (low + high) / 2;
        if (altitude(middle) > horizon) low = middle;
        else high = middle;
    }
    return high;
}

/** The clock time of an instant in the rider's own time zone. */
export const clockTime = (instant: number) => new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit' }).format(instant);

/** "Night 3–8 °C": the night lows of the week of `date` over the climate years at a point and height, or '' without
 * climate data there. */
export async function nightLow(coordinate: Coordinate, elevation: number | null, date: string, signal: AbortSignal): Promise<string> {
    const layer = dataLayers.find(layer => layer.nights);
    if (!layer?.nights || elevation === null) return '';
    const samples = await layer.sample({ coordinates: [coordinate], elevation: [elevation] }, signal, { date, theme: 'light' });
    return layer.nights(samples, [{ index: 0, date }])[0] ?? '';
}
