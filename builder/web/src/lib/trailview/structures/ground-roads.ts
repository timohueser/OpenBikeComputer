// PROTOTYPE (road-quality study): the roads and tracks on the ground as meshes. The road face lies on the
// line's C1 curve (`rebend` of bins.ts), painted from its metres across; a skirt on each side runs from
// the road's edge to just under the drawn surface and takes the ground's look. The stamp holds the
// terrain SINK_M under the road (stamp-core.wgsl), so the road mesh covers it.
import { BRIDGE, metresPerUnit, PATH, RAIL, ROAD, TUNNEL, type Line } from '../terrain/bins';
import { GROUND, PAINT, rgb, type MeshBuilder } from './mesh';
import type { Owns } from './node';

/** The skirt's width past the road's edge, how far its outer edge sinks under the drawn surface, and the longest step along a line. */
const SKIRT_M = 2.5, SINK_M = 0.08, STEP_M = 2;
/** No skirt this near a junction, past the half-width of its widest road: the skirts would cross the other roads. */
const JUNCTION_CLEAR_M = 4;
/** A face in this colour takes the look of the ground under it (structures.wgsl). */
export const GROUND_LOOK = rgb('#010203');

export function groundRoads(b: MeshBuilder, lines: Line[], owns: Owns): void {
    const arms = new Map<string, { x: number; y: number; n: number; h: number }>();
    for (const line of lines) {
        if (![ROAD, RAIL, BRIDGE, TUNNEL, PATH].includes(line.kind)) continue;
        const n = line.z.length;
        for (let v = 0; v < n; v++) {
            if (!(line.anchors?.[v] ?? true)) continue;
            const x = line.xy[2 * v], y = line.xy[2 * v + 1], k = `${x},${y}`, at = arms.get(k) ?? { x, y, n: 0, h: 0 };
            at.n += v === 0 || v === n - 1 ? 1 : 2;
            at.h = Math.max(at.h, line.h);
            arms.set(k, at);
        }
    }
    const junctions = [...arms.values()].filter(({ n }) => n >= 3);
    for (const line of lines) if (line.kind === ROAD || line.kind === RAIL) ribbon(b, line, owns, junctions);
}

/** The points of a line every STEP_M at most: its chords lie on its curve (`rebend` of bins.ts), so the skirts meet the ground often. */
function profile(line: Line): { x: number; y: number; z: number }[] {
    const n = line.z.length, out = [{ x: line.xy[0], y: line.xy[1], z: line.z[0] }];
    for (let v = 1; v < n; v++) {
        const metres = Math.hypot(line.xy[2 * v] - line.xy[2 * v - 2], line.xy[2 * v + 1] - line.xy[2 * v - 1]) * metresPerUnit(line.xy[2 * v + 1]);
        const steps = Math.max(1, Math.ceil(metres / STEP_M));
        for (let k = 1; k <= steps; k++) {
            const u = k / steps;
            out.push({ x: line.xy[2 * v - 2] + (line.xy[2 * v] - line.xy[2 * v - 2]) * u, y: line.xy[2 * v - 1] + (line.xy[2 * v + 1] - line.xy[2 * v - 1]) * u, z: line.z[v - 1] + (line.z[v] - line.z[v - 1]) * u });
        }
    }
    return out;
}

function ribbon(b: MeshBuilder, line: Line, owns: Owns, junctions: { x: number; y: number; h: number }[]): void {
    const points = profile(line), h = line.h, paint = rgb('#000000', PAINT) | line.code;
    const stations = points.map((p, j) => {
        const a = points[Math.max(0, j - 1)], c = points[Math.min(points.length - 1, j + 1)];
        const dx = c.x - a.x, dy = c.y - a.y, l = Math.hypot(dx, dy) || 1, m = metresPerUnit(p.y);
        const near = junctions.some(q => Math.hypot(q.x - p.x, q.y - p.y) * m < q.h + h + SKIRT_M + JUNCTION_CLEAR_M);
        return { ...p, ax: dy / l / m, ay: -dx / l / m, near };
    });
    for (let j = 0; j + 1 < stations.length; j++) {
        const s = stations[j], t = stations[j + 1];
        if (!owns(s.x, s.y)) continue;
        const at = (st: typeof s, a: number, colour: number, across = 0, half = 0) => b.vertex(st.x + st.ax * a, st.y + st.ay * a, st.z, colour, across, half);
        const ground = (st: typeof s, a: number) => b.vertex(st.x + st.ax * a, st.y + st.ay * a, -SINK_M, GROUND_LOOK | (GROUND << 24));
        b.quad([at(s, -h, paint, -h, h), at(s, h, paint, h, h), at(t, h, paint, h, h), at(t, -h, paint, -h, h)], [0, 0, 1]);
        if (s.near || t.near) continue;
        for (const side of [1, -1]) {
            const w = side * (h + SKIRT_M);
            b.quad([at(s, side * h, GROUND_LOOK), ground(s, w), ground(t, w), at(t, side * h, GROUND_LOOK)], [0, 0, 1]);
        }
    }
}
