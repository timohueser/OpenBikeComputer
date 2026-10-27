import { readFileSync } from 'node:fs';
import { gunzipSync } from 'node:zlib';
import { describe, expect, it } from 'vitest';
import { decodeFeatures, type Feature } from '../stream/features';
import { features, metresPerUnit, TUNNEL, type Line } from '../terrain/bins';
import { portalBox } from './tunnels';

/** A zoom 13 tile at Engelberg: its footway tunnel steepens inside the boxes of its portals. About 0.2 m per tile unit. */
const TILE = { level: 13, x: 4287, y: 2887 };
const tunnel = (list: Feature[]) => features(list, TILE).lines.find(line => line.kind === TUNNEL && line.deck! < 3)!;
const way = (kind: string, tags: Record<string, string>, xy: number[], z: number[]): Feature => ({ kind, tags, geometry: 'line', parts: [{ outer: false, xy: Int32Array.from(xy), z: Float32Array.from(z) }] });
const metres = (line: Line, v: number, w: number) => Math.hypot(line.xy[2 * w] - line.xy[2 * v], line.xy[2 * w + 1] - line.xy[2 * v + 1]) * metresPerUnit(line.xy[2 * v + 1]);

/** The box of each portal keeps its foot under the bore's floor and its roof over the bore's crown at each vertex in it. */
function expectAroundBore(line: Line) {
    const n = line.z.length;
    for (const [from, to] of [[0, n - 1], [n - 1, 0]]) {
        const [depth, , roof, foot, rise] = portalBox(line, from, to), step = Math.sign(to - from);
        for (let v = from, s = 0; v !== to; v += step) {
            s += metres(line, v, v + step);
            if (s > depth) break;
            expect(foot + rise * s - line.z[v + step]).toBeLessThanOrEqual(foot - line.z[from] + 1e-6);
            expect(roof + rise * s - line.z[v + step] - line.deck!).toBeGreaterThanOrEqual(-1e-6);
        }
    }
}

describe('the box of a portal', () => {
    it('keeps its foot under the bore floor and its roof over the crown, and ends at the other end of a short tunnel where that is a portal', () => {
        expectAroundBore(tunnel(decodeFeatures(new Uint8Array(gunzipSync(readFileSync(new URL(`../../../../tests/browser/trailview/fixture/features/13/${TILE.x}/${TILE.y}.bin`, import.meta.url)))))));
        // Flat for a metre, then 15 % up, 10 m long: a path meets the ground at its first end, and at the other end in `both`.
        const bore = way('tunnel', { highway: 'footway', tunnel: 'yes' }, [8000, 8000, 8005, 8000, 8049, 8000], [1000, 1000, 1001.35]);
        const before = way('path', { highway: 'footway' }, [7950, 8000, 8000, 8000], [1000, 1000]), after = way('path', { highway: 'footway' }, [8049, 8000, 8100, 8000], [1001.35, 1001.35]);
        const both = tunnel([bore, before, after]), one = tunnel([bore, before]), length = metres(both, 0, 1) + metres(both, 1, 2);
        expectAroundBore(both);
        expect(portalBox(both, 0, 2)[0]).toBeCloseTo(length, 6);
        expect(portalBox(one, 0, 2)[0]).toBeGreaterThan(length + 1);
    });
});
