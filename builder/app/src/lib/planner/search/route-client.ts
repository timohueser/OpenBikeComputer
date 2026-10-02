import type { Coordinate } from '../editor';
import { decodeRoutes } from '../route-answer';
import type { ResolvedPoint } from './types';
import { SEARCH_URL } from './config';

/** One line for each pair of points. A leg whose road ends away from its point gets a straight, unverified connector to it. */
export async function buildQueryRoute(points: ResolvedPoint[], bike: string, goal: string, onNotice?: (notice: string) => void): Promise<Coordinate[][]> {
    const response = await fetch(`${SEARCH_URL}/route`, { method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ points: points.map(p => p.coordinate), bike, goal }) });
    const result = await response.json();
    if (!response.ok) throw new Error(result.error ?? 'The routing engine could not find a route.');
    const [route] = decodeRoutes(result);
    if (route?.legs.length !== points.length - 1 || route.legs.some((leg, i) => leg.from_index !== (i ? route.legs[i - 1].to_index : 0)
        || leg.to_index <= leg.from_index || leg.to_index >= route.geometry.length))
        throw new Error('The routing engine returned incomplete legs.');
    const warnings = new Set(route.snap_truncated ? ['The routing engine reached its snapping search limit.'] : []);
    const legs = route.legs.map(({ from_index, to_index }, i) => {
        const line = route.geometry.slice(from_index, to_index + 1);
        const [from, to] = [points[i].coordinate, points[i + 1].coordinate];
        if (from[0] !== line[0][0] || from[1] !== line[0][1]) line.unshift(from);
        if (to[0] !== line.at(-1)![0] || to[1] !== line.at(-1)![1]) line.push(to);
        if (line.length > to_index - from_index + 1) warnings.add('Connections from the selected places to snapped roads are straight and unverified.');
        return line;
    });
    if (warnings.size) onNotice?.([...warnings].join(' '));
    return legs;
}
