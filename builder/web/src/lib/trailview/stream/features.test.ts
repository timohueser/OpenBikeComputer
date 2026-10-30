import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { decodeFeatures, type Feature } from './features';

const vector = (name: string) => readFileSync(new URL(`../../../../../../specs/vectors/${name}`, import.meta.url));

const codes = (values: Float32Array, scale: number) => Array.from(values, value => Math.round(value * scale));

/** A decoded feature in the form of the shared vector: arrays, heights in centimetres, null where not solved, alignments in codes. */
const plain = ({ kind, tags, geometry, parts }: Feature) => ({
    kind, tags, geometry,
    parts: parts.map(({ outer, xy, z, plan }) => ({
        outer,
        xy: Array.from({ length: xy.length / 2 }, (_, v) => [xy[2 * v], xy[2 * v + 1]].map(value => (xy instanceof Float32Array ? value * 4 : value))),
        ...(z ? { z: Array.from(z, h => (Number.isNaN(h) ? null : Math.round(h * 100) / 100)) } : {}),
        ...(plan ? {
            along: Math.round(plan.along * 10) * 2 + (plan.backward ? 1 : 0),
            tangent: codes(plan.tangent, 1024 / (2 * Math.PI)), grade: codes(plan.grade, 250), bank: codes(plan.bank, 1000), widen: codes(plan.widen, 100),
        } : {}),
    })),
});

describe('feature tiles', () => {
    const bytes = new Uint8Array(vector('trailview-features.bin'));

    it('decode the shared vector to the features that the encoder read, and reject a tile that ends early or has bytes after its last feature', () => {
        expect(decodeFeatures(bytes).map(plain)).toEqual(JSON.parse(vector('trailview-features.json').toString()));
        expect(() => decodeFeatures(bytes.subarray(0, bytes.length - 1))).toThrow('ends inside');
        expect(() => decodeFeatures(new Uint8Array([...bytes, 0]))).toThrow('after its last feature');
    });
});
