import type { Coordinate } from './geo';
import { config } from './map-data';
import { releaseFetch } from './release';
import { searchPlaces } from './search/types';

export const coordinateName = (coordinate: Coordinate) => `${coordinate[1].toFixed(5)}, ${coordinate[0].toFixed(5)}`;

export async function visitName(coordinate: Coordinate): Promise<string | null> {
    try {
        const response = await releaseFetch(`${config.search}/reverse`, { method: 'POST', headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ coordinate }), signal: AbortSignal.timeout(2000) });
        if (response.ok) {
            const { label } = await response.json();
            if (typeof label === 'string' && label.trim()) return label;
        }
    } catch { /* Coordinates remain usable when search is unavailable. */ }
    return null;
}

const towns = ['city', 'town', 'village'];

/** The nearest city, town or village within 3 km of `coordinate`, from place search; null after 2 s or on a failure. */
export async function townName(coordinate: Coordinate): Promise<string | null> {
    const [lon, lat] = coordinate;
    try {
        // A radius search centres on the middle of the view.
        const answer = await searchPlaces('', { view: [lon - .01, lat - .01, lon + .01, lat + .01], plan: { coordinates: [], km: [], days: [], points: [] } },
            6, AbortSignal.timeout(2000), { type: 'places', what: ['town'], radius: { value: 3, unit: 'km' } });
        return answer.results?.filter(place => towns.includes(place.kind)).sort((a, b) => a.distance - b.distance)[0]?.name ?? null;
    } catch { return null; }
}
