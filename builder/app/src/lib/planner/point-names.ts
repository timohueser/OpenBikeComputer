import type { Coordinate } from './editor';
import { SEARCH_URL } from './search/config';

export const coordinateName = (coordinate: Coordinate) => `${coordinate[1].toFixed(5)}, ${coordinate[0].toFixed(5)}`;

export async function visitName(coordinate: Coordinate, region: string): Promise<string | null> {
    try {
        const response = await fetch(`${SEARCH_URL}/reverse`, { method: 'POST', headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ coordinate, region }), signal: AbortSignal.timeout(2000) });
        if (response.ok) {
            const { label } = await response.json();
            if (typeof label === 'string' && label.trim()) return label;
        }
    } catch { /* Coordinates remain usable when search is unavailable. */ }
    return null;
}
