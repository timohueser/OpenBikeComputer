// The galleries of a node: over the road of each gallery line, a wall on the side of the hill, a roof that
// slopes down to the valley, and pillars on the open side. The road under it is the road on the ground.
import { metresPerUnit, ROAD, type Line } from '../terrain/bins';
import { MeshBuilder, posts, rgb, station, type Corner, type Station } from './mesh';

/** A gallery of a road and of a path: its clear height, the margin of its walls past the road, its pillars' spacing. */
const ROAD_GALLERY = { height: 5, margin: 0.4, spacing: 4.5 }, PATH_GALLERY = { height: 2.8, margin: 0.2, spacing: 3 };
/** The walls' and the pillars' thickness, the roof's thickness at the hill and at the valley, how far it reaches into the hill, and how deep the walls stand in the ground. */
const WALL_M = 0.6, ROOF_M = [1.2, 0.6], TUCK_M = 1.5, SINK_M = 0.5;

const WALL = rgb('#a19e95'), ROOF_TOP = rgb('#8c8a83'), ROOF_UNDER = rgb('#5e5c57'), PILLAR = rgb('#aeaba2');

export function galleries(b: MeshBuilder, lines: Line[], box: [number, number, number, number]): void {
    for (const line of lines) {
        if (!line.gallery) continue;
        const n = line.z.length, m = (line.h + 10) / metresPerUnit(line.xy[1]), size = line.kind === ROAD ? ROAD_GALLERY : PATH_GALLERY;
        const stations = new Map<number, Station>(), at = (v: number) => stations.get(v) ?? stations.set(v, station(line, v)).get(v)!;
        let along = 0;
        for (let v = 0; v + 1 < n; v++) {
            const xs = [line.xy[2 * v], line.xy[2 * v + 2]], ys = [line.xy[2 * v + 1], line.xy[2 * v + 3]];
            const length = Math.hypot(xs[1] - xs[0], ys[1] - ys[0]) * metresPerUnit(ys[0]);
            if (Math.max(...xs) + m >= box[0] && Math.min(...xs) - m <= box[2] && Math.max(...ys) + m >= box[1] && Math.min(...ys) - m <= box[3]) {
                segment(b, at(v), at(v + 1), line.h, line.gallery, size, along, length);
            }
            along += length;
        }
    }
}

/** A gallery from station s to t over a road of half-width h, with the hill on `side` (1 left). */
function segment(b: MeshBuilder, s: Station, t: Station, h: number, side: 1 | -1, size: typeof ROAD_GALLERY, along: number, length: number): void {
    const inner = h + size.margin, outer = inner + WALL_M, top = size.height, [hill, valley] = ROOF_M;
    const c = (across: number, up: number): Corner => [side * across, up];
    // The wall against the hill, its face to the road and its top under the roof.
    b.strip(s, t, c(inner, -SINK_M), c(inner, top), WALL, [-side, 0]);
    // The roof: its underside over the road, its top from the hill down to the valley, its edge on the valley side.
    b.strip(s, t, c(outer + TUCK_M, top), c(-outer, top), ROOF_UNDER, [0, -1]);
    b.strip(s, t, c(-outer, top + valley), c(outer + TUCK_M, top + hill), ROOF_TOP, [0, 1]);
    b.strip(s, t, c(-outer, top), c(-outer, top + valley), ROOF_TOP, [-side, 0]);
    // A pillar every so many metres on the valley side.
    posts(s, t, along, length, (p, q) => {
        for (const [a, o] of [[inner, side], [outer, -side]] as [number, number][]) b.strip(p, q, c(-a, -SINK_M), c(-a, top), PILLAR, [o, 0]);
        for (const [st, sign] of [[p, -1], [q, 1]] as [Station, number][]) {
            const dx = t.x - s.x, dy = t.y - s.y, l = Math.hypot(dx, dy) || 1;
            b.quad([b.at(st, c(-inner, -SINK_M), PILLAR), b.at(st, c(-outer, -SINK_M), PILLAR), b.at(st, c(-outer, top), PILLAR), b.at(st, c(-inner, top), PILLAR)], [sign * dx / l, -sign * dy / l, 0]);
        }
    }, WALL_M, size.spacing);
}
