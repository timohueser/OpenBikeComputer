import type { Coordinate } from '../editor';
import type { ResolvedPoint } from './types';
import { SEARCH_URL } from './config';

export async function buildQueryRoute(points: ResolvedPoint[], bike: string, goal: string, onNotice?: (notice: string) => void): Promise<Coordinate[][]> {
    const response = await fetch(`${SEARCH_URL}/route`, { method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ points: points.map(p => p.coordinate), bike, goal }) });
    const result = await response.json();
    if (!response.ok) throw new Error(result.error ?? 'The routing engine could not find a route.');
    if (result.warnings?.length) onNotice?.(result.warnings.join(' '));
    return result.legs;
}
