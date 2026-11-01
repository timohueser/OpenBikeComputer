import { readdirSync, readFileSync } from 'node:fs';
import { gunzipSync } from 'node:zlib';
import { describe, expect, it } from 'vitest';
import { decodeFeatures, DEEPEST, type Feature, type FeaturePart } from '../stream/features';
import { features, metresPerUnit, ROAD, type Line } from '../terrain/bins';
import { OWNER } from '../terrain/lod';
import { structureMeshes } from '../structures/node';
import { decksOf, isDeck } from './decks';
import { planOf, roadMesh } from './mesh';
import { CUT } from './stations';
import { countOf } from './sweep';

const root = new URL('../../../../tests/browser/trailview/fixture/features/', import.meta.url);
const tiles = readdirSync(new URL(`${DEEPEST}/`, root)).flatMap(x => readdirSync(new URL(`${DEEPEST}/${x}/`, root)).map(file => ({
    tile: { level: DEEPEST, x: Number(x), y: parseInt(file, 10) }, list: decodeFeatures(new Uint8Array(gunzipSync(readFileSync(new URL(`${DEEPEST}/${x}/${file}`, root))))),
})));

/** World units of `i10-intersection`'s bridge, by its point in degrees. */
const worldOf = (lat: number, lon: number) => [(lon + 180) / 360 * 2 ** 30, (1 - Math.log(Math.tan(Math.PI / 4 + lat * Math.PI / 360)) / Math.PI) / 2 * 2 ** 30];

/** The deck of the bridge in `i10-intersection` with what meets its ends, in a tile that holds it. */
function townDeck() {
    const [x, y] = worldOf(46.81847, 8.397);
    for (const { tile, list } of tiles) {
        const shapes = features(list, tile), line = shapes.lines.find(l => isDeck(l) && Math.hypot(l.xy[0] - x, l.xy[1] - y) * metresPerUnit(y) < 30);
        const near = line && shapes.lines.filter(o => o !== line && o.kind === ROAD && [0, o.z.length - 1].some(v => [0, line.z.length - 1].some(w => o.xy[2 * v] === line.xy[2 * w] && o.xy[2 * v + 1] === line.xy[2 * w + 1])));
        if (line && near!.length >= 3) return { shapes, line, near: near!, plan: planOf(shapes) };
    }
    throw new Error('the fixture has lost the bridge of i10-intersection');
}

describe('a road deck', () => {
    it('is a way of its junction: the side road at its end has a mouth from the deck, and the deck stays through', () => {
        const { line, plan: { junctions: { mouths } } } = townDeck(), mouth = mouths.find(({ through }) => through.some(way => way.line === line));
        expect(mouth).toBeDefined();
        expect(isDeck(mouth!.way.line)).toBe(false);
    });

    it('runs its dashes on across both its ends', () => {
        const { line, near, plan: { runs } } = townDeck(), run = (l: Line) => runs.find(r => r.line === l)!, deck = run(line);
        const ends = (l: Line) => [run(l).stations[0], run(l).stations[run(l).stations.length - 1]];
        const onward = near.filter(({ code, profile }) => code === line.code && profile === line.profile);
        expect(onward.length).toBe(2);
        for (const s of ends(line)) {
            const road = onward.find(l => ends(l).some(t => t.x === s.x && t.y === s.y))!, t = ends(road).find(o => o.x === s.x && o.y === s.y)!;
            expect(Math.abs(countOf({ ...deck, base: 0 }, s) - countOf({ ...run(road), base: 0 }, t))).toBeLessThan(0.2);
        }
    });

    it('takes the sidewalks of the road that runs on from it, which reach its ends, and gives way on the side of the road that leaves', () => {
        const { shapes: { lines }, line, near } = townDeck(), { walks, yields } = decksOf(lines).get(line)!;
        expect(walks.every(walk => walk > 0)).toBe(true);
        for (const road of near.filter(({ profile }) => profile === line.profile)) expect(road.sidewalks!.every(side => side === 3)).toBe(true);
        expect(yields.flat().filter(d => d > 0).length).toBe(1);
    });

    it('leaves its walk out on the side that gives way, and has no band', () => {
        const { line, plan: { runs } } = townDeck(), { stations } = runs.find(r => r.line === line)!;
        expect(stations.some(({ cut }) => (cut ?? 0) & (CUT.LEFT | CUT.RIGHT))).toBe(true);
        expect(stations.every(({ cut }) => !((cut ?? 0) & CUT.ROAD))).toBe(true);
    });
});

/** A straight line of `n` vertices 40 units apart from x, as the bake gives it, with its stroke's metres. */
function part(x: number, n: number, along: number): FeaturePart {
    const column = (value: number) => new Float32Array(n).fill(value);
    return {
        outer: false, xy: Float32Array.from({ length: 2 * n }, (_, i) => (i % 2 ? 1000 : x + (40 * i) / 2)), z: column(1000),
        plan: { along, backward: false, tangent: column(0), grade: column(0), bank: column(0), widen: column(0), side: new Uint8Array(n) },
    } as unknown as FeaturePart;
}

describe('a road tunnel', () => {
    /** A secondary road that runs into a tunnel at its portal, in a tile of the fixture's level. */
    const list = [
        { kind: 'main-road', tags: { highway: 'secondary', surface: 'asphalt' }, parts: [part(1000, 4, 0)] },
        { kind: 'tunnel', tags: { highway: 'secondary', surface: 'asphalt', tunnel: 'yes' }, parts: [part(1120, 4, 120)] },
    ] as unknown as Feature[], tile = { level: DEEPEST, x: 0, y: 0 }, owner = { level: OWNER, x: 0, y: 0 };

    it('has no round end on its road at the portal, whose corridor ends where the bore starts', () => {
        const { lines } = features(list, tile), road = lines.find(({ kind }) => kind === ROAD)!, { runs } = planOf(features(list, tile));
        const run = runs.find(r => r.line === road)! as (typeof runs)[number];
        expect(run.caps.map(({ j }) => j)).toEqual([0]);
        expect(roadMesh(list, tile, owner).pieces.length).toBeGreaterThan(0);
        const { far: { structures } } = structureMeshes(list, tile, owner, true);
        expect(structures).not.toBeNull();
        const words = new Int32Array(structures!.vertices), last = run.stations[run.stations.length - 1];
        let nearest = Infinity;
        for (let i = 0; i < words.length; i += 6) nearest = Math.min(nearest, Math.hypot(words[i] - last.x, words[i + 1] - last.y));
        expect(nearest * metresPerUnit(last.y)).toBeLessThan(road.h + 1);
    });
});
