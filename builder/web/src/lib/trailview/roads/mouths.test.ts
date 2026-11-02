import { readFileSync, readdirSync } from 'node:fs';
import { gunzipSync } from 'node:zlib';
import { describe, expect, it } from 'vitest';
import { decodeFeatures, type Feature } from '../stream/features';
import { features, metresPerUnit, ROAD, SHOULDER_M, STREAM } from '../terrain/bins';
import { cornerAt } from '../terrain/corners';
import { GIVE_WAY_M, MOUTH_BACK_M, MOUTH_HALF_M, waysOf, type CulvertMouth } from '../terrain/culverts';

/** A zoom 13 tile at Engelberg, whose tile unit is about 0.2 m; and the world units of a tile unit. */
const TILE = { level: 13, x: 4288, y: 2887 }, SCALE = 2 ** 17 / 16384;
const part = (xy: number[], z: number[] | null) => ({ outer: false, xy: new Int32Array(xy), z: z && new Float32Array(z) });
const stream = (xy: number[]) => ({ kind: 'stream', tags: { waterway: 'stream' }, geometry: 'line', parts: [part(xy, xy.filter((_, i) => i % 2 === 0).map((_, i) => 1000 - i))] }) as Feature;
const culvert = (xy: number[]) => ({ kind: 'stream', tags: { waterway: 'stream', tunnel: 'culvert' }, geometry: 'line', parts: [part(xy, null)] }) as Feature;
/** A way along y = 8000, 100 m long. */
const way = (tags: Record<string, string> = { highway: 'track' }, kind = 'road') => ({ kind, tags, geometry: 'line', parts: [part([7750, 8000, 8250, 8000], [1000, 1000])] }) as Feature;
const shapes = (list: Feature[]) => features(list, TILE);
const streams = (list: Feature[]) => shapes(list).lines.filter(({ kind }) => kind === STREAM);
/** The metres of each mouth north of the way's middle, south negative, and its front's north. */
const across = (list: Feature[]) => {
    const y = (TILE.y * 16384 + 8000) * SCALE;
    return shapes(list).mouths.map(m => ({ north: (y - m.y) * metresPerUnit(m.y), front: Math.round(-m.uy) }));
};

describe('the mouths of a stream', () => {
    it('stand on both sides of a road that it passes under, tagged or not, fronts along the stream, and its ribbon ends there', () => {
        for (const list of [[way(), stream([8000, 7900, 8000, 8100])], [way(), stream([8000, 7900, 8000, 7970]), culvert([8000, 7970, 8000, 8030]), stream([8000, 8030, 8000, 8100])]]) {
            const half = shapes(list).lines.find(({ kind }) => kind === ROAD)!.h, mouths = across(list).sort((a, b) => a.north - b.north);
            expect(mouths.map(({ front }) => front)).toEqual([-1, 1]);
            for (const { north } of mouths) {
                expect(Math.abs(north) - half).toBeGreaterThanOrEqual(GIVE_WAY_M);
                expect(Math.abs(north) - half).toBeLessThan(GIVE_WAY_M + 0.2);
            }
            expect(streams(list)).toHaveLength(2);
        }
    });

    it('do not stand at a bridge, at a ford, beside a way that the stream does not cross, or at a foot way', () => {
        for (const way_ of [way({ highway: 'track', bridge: 'yes' }), way({ highway: 'track', ford: 'yes' }), way({ highway: 'path' }, 'path')]) expect(across([way_, stream([8000, 7900, 8000, 8100])])).toEqual([]);
        expect(across([way(), stream([7700, 7985, 8100, 7985])])).toEqual([]);
    });

    it('stand where a line that stops short of a way runs on to it from ahead, whichever way it is drawn', () => {
        for (const line of [[8000, 7900, 8000, 7985], [8000, 7985, 8000, 7900]]) expect(across([way(), stream(line)])).toHaveLength(1);
        // Past RUN_ON_M the line ends where it ends, and more than about 45° off the way it does not run on.
        expect(across([way(), stream([8000, 7900, 8000, 7950])])).toEqual([]);
        expect(across([way(), stream([7900, 7985, 8000, 7985])])).toEqual([]);
    });

    it('stand clear of the way where a culvert that runs along it buries the stream, which is cut back along its line', () => {
        const beside = [way(), stream([7900, 7900, 7900, 7985]), culvert([7900, 7985, 8000, 7985]), stream([8000, 7985, 8000, 7900])];
        expect(streams(beside)).toHaveLength(2);
        const half = shapes(beside).lines.find(({ kind }) => kind === ROAD)!.h;
        for (const { north, front } of across(beside)) {
            expect(front).toBe(1);
            expect(north - half).toBeGreaterThanOrEqual(GIVE_WAY_M);
        }
        expect(across(beside)).toHaveLength(2);
        const long = [stream([1000, 1000, 2000, 1000]), culvert([2000, 1000, 2500, 1000]), stream([2500, 1000, 3000, 1000])];
        expect(shapes(long).mouths).toHaveLength(2);
    });
});

describe('the mouths of the fixture', () => {
    const dir = new URL('../../../../tests/browser/trailview/fixture/features/13/', import.meta.url);
    const tiles = readdirSync(dir).flatMap(x => readdirSync(new URL(`${x}/`, dir)).map(f => ({ level: 13, x: Number(x), y: Number(f.replace('.bin', '')) })));
    const side = 2 ** 17, all = tiles.map(tile => ({ at: tile, ...features(decodeFeatures(new Uint8Array(gunzipSync(readFileSync(new URL(`${tile.x}/${tile.y}.bin`, dir))))), tile) }));
    const owned = (tile: (typeof tiles)[number], { x, y }: CulvertMouth) => Math.floor(x / side) === tile.x && Math.floor(y / side) === tile.y;

    it('stand clear of every way and corner, each wall whole', () => {
        const widest = Math.max(...Object.values(SHOULDER_M));
        let count = 0;
        for (const { lines, mouths, at: tile } of all) for (const mouth of mouths) {
            if (!owned(tile, mouth)) continue;
            count++;
            const { x, y, ux, uy } = mouth, unit = metresPerUnit(y);
            for (const a of [-MOUTH_HALF_M, 0, MOUTH_HALF_M]) for (const b of [0, MOUTH_BACK_M]) {
                const px = x + (-uy * a - ux * b) / unit, py = y + (ux * a - uy * b) / unit;
                expect(waysOf(lines).gap(px, py)?.metres ?? Infinity).toBeGreaterThanOrEqual(widest);
                expect(cornerAt(lines, px, py)).toBeUndefined();
            }
        }
        expect(count).toBeGreaterThan(100);
    });

    it('land on the same place in each tile that holds them, up to 30 m past the tile', () => {
        let shared = 0;
        for (const { at: tile, mouths } of all) for (const mouth of mouths) {
            if (!owned(tile, mouth)) continue;
            const unit = metresPerUnit(mouth.y);
            for (const { at: other, mouths: there } of all) {
                if (other === tile || Math.max(other.x * side - mouth.x, mouth.x - (other.x + 1) * side, other.y * side - mouth.y, mouth.y - (other.y + 1) * side) * unit > 30) continue;
                const twin = there.find(o => Math.hypot(o.x - mouth.x, o.y - mouth.y) * unit < 3);
                expect(twin).toBeDefined();
                expect(Math.hypot(twin!.x - mouth.x, twin!.y - mouth.y) * unit).toBeLessThan(0.01);
                shared++;
            }
        }
        expect(shared).toBeGreaterThan(0);
    });
});
