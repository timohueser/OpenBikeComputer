import type { Coordinate } from '../../src/lib/planner/editor';
import type { AnswerRoute } from '../../src/lib/planner/route-answer';
import type { RouteTotals } from '../../src/lib/planner/routing';

const totals = (metres: number): RouteTotals =>
    ({ distance_m: metres, ascent_m: 0, seconds: metres, surface_m: [0, metres, 0, 0, 0, 0], unknown_elevation_m: metres, pushing_m: 0 });

/** A `fetch` for `/v1/route` whose route is the straight line through the request points, 1 km and 1000 s for each leg.
 * A leg position names the package and its point. */
export function routeService(identity = 'test') {
    return async (_url: string, init: RequestInit) => {
        const { points, profile } = JSON.parse(init.body as string) as { points: Coordinate[]; profile: string };
        const edges = points.length - 1;
        const legs = points.slice(1).map((point, k) => ({ from_index: k, to_index: k + 1, start: `${identity}:${points[k]}`, end: `${identity}:${point}`, totals: totals(1000) }));
        const coordinates_udeg = points.flat().map((value, i, flat) => Math.round(value * 1e6) - (i > 1 ? Math.round(flat[i - 2] * 1e6) : 0));
        const routes: AnswerRoute[] = [{ id: 'whole', reason: 'primary', package: identity, profile, coordinates_udeg, elevation_dm: points.map(() => null),
            elapsed_s: points.map((_, i) => i && 1000), surfaces: [['Paved', edges]], pushing: [[false, edges]], closures: [[null, edges]], legs, snap_truncated: false, totals: totals(1000 * edges) }];
        return { ok: true, json: async () => ({ routes }) };
    };
}
