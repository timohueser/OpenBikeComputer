import type { Coordinate } from './editor';
import type { EngineRoute } from './routing';

/** One route as the routing service sends it: integer deltas and runs, as `specs/route-api.md` specifies. */
export interface AnswerRoute extends Omit<EngineRoute, 'geometry' | 'elevation' | 'elapsed' | 'edges'> {
    coordinates_udeg: number[];
    elevation_dm: (number | null)[];
    elapsed_s: number[];
    edges?: Record<string, [unknown, number][]>;
}

const count = (runs: [unknown, number][]) => runs.reduce((sum, [, length]) => sum + length, 0);

function decodeRoute({ coordinates_udeg, elevation_dm, elapsed_s, edges = {}, ...route }: AnswerRoute): EngineRoute {
    const geometry: Coordinate[] = [];
    for (let i = 0, lon = 0, lat = 0; i < coordinates_udeg.length; i += 2) {
        lon += coordinates_udeg[i];
        lat += coordinates_udeg[i + 1];
        geometry.push([lon / 1e6, lat / 1e6]);
    }
    let height = 0;
    let time = 0;
    return {
        ...route, geometry,
        elevation: elevation_dm.map(delta => delta === null ? null : (height += delta) / 10),
        elapsed: elapsed_s.map(delta => time += delta),
        edges: Object.fromEntries(Object.entries(edges).map(([channel, runs]) =>
            [channel, runs.flatMap(([value, length]) => Array(length).fill(value))])),
    };
}

/** The routes of an answer. An answer whose arrays disagree in length is rejected before it is expanded. */
export function decodeRoutes(data: { routes?: AnswerRoute[] }): EngineRoute[] {
    if (!Array.isArray(data.routes)) throw new Error('The routing service returned an invalid response.');
    return data.routes.map(route => {
        const points = route.coordinates_udeg?.length / 2;
        if (!Number.isInteger(points) || points < 1 || route.elevation_dm?.length !== points || route.elapsed_s?.length !== points
            || Object.values(route.edges ?? {}).some(runs => !Array.isArray(runs) || count(runs) !== points - 1))
            throw new Error('The routing service returned an invalid response.');
        return decodeRoute(route);
    });
}
