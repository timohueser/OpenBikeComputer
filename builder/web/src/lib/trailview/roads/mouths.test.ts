import { describe, expect, it } from 'vitest';
import type { Feature } from '../stream/features';
import { features, metresPerUnit, ROAD, STREAM } from '../terrain/bins';
import { mouthsOf } from './mouths';

/** A zoom 13 tile at Engelberg, whose tile unit is about 0.2 m; and the world units of a tile unit. */
const TILE = { level: 13, x: 4288, y: 2887 }, SCALE = 2 ** 17 / 16384;
const part = (xy: number[], z: number[] | null) => ({ outer: false, xy: new Int32Array(xy), z: z && new Float32Array(z) });
const stream = (xy: number[]) => ({ kind: 'stream', tags: { waterway: 'stream' }, geometry: 'line', parts: [part(xy, xy.filter((_, i) => i % 2 === 0).map((_, i) => 1000 - i))] }) as Feature;
const culvert = (xy: number[]) => ({ kind: 'stream', tags: { waterway: 'stream', tunnel: 'culvert' }, geometry: 'line', parts: [part(xy, null)] }) as Feature;
/** A track along y = 8000, 100 m long. */
const way = (tags: Record<string, string> = {}) => ({ kind: 'road', tags: { highway: 'track', ...tags }, geometry: 'line', parts: [part([7750, 8000, 8250, 8000], [1000, 1000])] }) as Feature;
const shapes = (list: Feature[]) => features(list, TILE);
/** The metres of each mouth north of the way's middle, south negative, and its front's north. */
const across = (list: Feature[]) => {
    const { lines } = shapes(list), y = (TILE.y * 16384 + 8000) * SCALE, half = lines.find(({ kind }) => kind === ROAD)?.h ?? 0;
    return mouthsOf(shapes(list)).map(m => ({ north: (y - m.y) * metresPerUnit(m.y), front: Math.round(-Math.cos(m.yaw)), half }));
};

describe('the mouths of a stream', () => {
    it('stand on both sides of a way that it passes under, tagged or not, past the ribbon\'s stop, their fronts away from it', () => {
        for (const list of [[way(), stream([8000, 7900, 8000, 8100])], [way(), stream([8000, 7900, 8000, 7970]), culvert([8000, 7970, 8000, 8030]), stream([8000, 8030, 8000, 8100])]]) {
            const mouths = across(list).sort((a, b) => a.north - b.north);
            expect(mouths.map(({ front }) => front)).toEqual([-1, 1]);
            for (const { north, half } of mouths) {
                expect(Math.abs(north) - half).toBeGreaterThanOrEqual(0.5);
                expect(Math.abs(north) - half).toBeLessThan(0.8);
            }
        }
    });

    it('do not stand at a bridge or at a ford', () => {
        expect(across([way({ bridge: 'yes' }), stream([8000, 7900, 8000, 8100])])).toEqual([]);
        expect(across([way({ ford: 'yes' }), stream([8000, 7900, 8000, 8100])])).toEqual([]);
    });

    it('stand where a line that stops short of a way runs on to it, whichever way it is drawn', () => {
        const short = shapes([way(), stream([8000, 7900, 8000, 7985])]).lines.find(({ kind }) => kind === STREAM)!;
        expect(short.xy.length / 2).toBe(3);
        expect(across([way(), stream([8000, 7900, 8000, 7985])])).toHaveLength(1);
        expect(shapes([way(), stream([8000, 7985, 8000, 7900])]).lines.find(({ kind }) => kind === STREAM)!.xy.length / 2).toBe(3);
        // Past RUN_ON_M the line ends where it ends.
        expect(across([way(), stream([8000, 7900, 8000, 7950])])).toEqual([]);
    });

    it('stand at the ends of a culvert that runs along a way, which buries the stream, and of a long one', () => {
        const beside = [way(), stream([7900, 7900, 7900, 7985]), culvert([7900, 7985, 8000, 7985]), stream([8000, 7985, 8000, 7900])];
        expect(shapes(beside).lines.filter(({ kind }) => kind === STREAM)).toHaveLength(2);
        expect(shapes(beside).culvertEnds).toHaveLength(2);
        expect(mouthsOf(shapes(beside))).toHaveLength(2);
        const long = [stream([1000, 1000, 2000, 1000]), culvert([2000, 1000, 2500, 1000]), stream([2500, 1000, 3000, 1000])];
        expect(mouthsOf(shapes(long))).toHaveLength(2);
    });
});
