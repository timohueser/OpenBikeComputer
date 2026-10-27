import { readFileSync } from 'node:fs';
import { gunzipSync } from 'node:zlib';
import { describe, expect, it } from 'vitest';
import { decodeFeatures, type Feature } from '../stream/features';
import { features, metresPerUnit, TUNNEL, type Line } from '../terrain/bins';
import { portalBox } from './tunnels';

/** A zoom 13 tile at Engelberg: its footway tunnel steepens inside the boxes of its portals. 8 world units per tile unit. */
const TILE = { level: 13, x: 4287, y: 2887 };
const tunnel = (list: Feature[]) => features(list, TILE).lines.find(line => line.kind === TUNNEL && line.deck! < 3)!;
const metres = (line: Line, v: number, w: number) => Math.hypot(line.xy[2 * w] - line.xy[2 * v], line.xy[2 * w + 1] - line.xy[2 * v + 1]) * metresPerUnit(line.xy[2 * v + 1]);

describe('the box of a portal', () => {
    it('keeps its foot under the bore floor and its roof over the bore, and ends at the other portal of a short tunnel', () => {
        const line = tunnel(decodeFeatures(new Uint8Array(gunzipSync(readFileSync(new URL(`../../../../tests/browser/trailview/fixture/features/13/${TILE.x}/${TILE.y}.bin`, import.meta.url))))));
        const n = line.z.length;
        for (const [from, to] of [[0, n - 1], [n - 1, 0]]) {
            const [depth, , roof, foot, rise] = portalBox(line, from, to), step = Math.sign(to - from);
            for (let v = from, s = 0; v !== to; v += step) {
                s += metres(line, v, v + step);
                if (s > depth) break;
                expect(foot + rise * s - line.z[v + step]).toBeLessThanOrEqual(foot - line.z[from] + 1e-9);
                expect(roof + rise * s).toBeGreaterThan(line.z[v + step] + line.deck!);
            }
        }
        const short = tunnel([{ kind: 'tunnel', tags: { highway: 'footway', tunnel: 'yes' }, geometry: 'line', parts: [{ outer: false, xy: Int32Array.from([8000, 8000, 8040, 8000]), z: Float32Array.from([1000, 1001]) }] }]);
        expect(portalBox(short, 0, 1)[0]).toBeCloseTo(metres(short, 0, 1), 6);
    });
});
