import { describe, expect, it } from 'vitest';
import { linear } from '../ground/rules';
import type { Look, Region } from '../rules/regions';
import { eyeLook } from './light';

const look = (hazeKm: number, horizon?: string): Look => ({
    snap: 0.4, hazeKm, hazeHeight: 2500, mistTop: 0, softness: 0.08, gradient: 0.35, ramps: {}, outcrops: 0, snow_share: 0.5, turf_deg: 42,
    rock: '#8c877d', sea: { deep: '#2c5874', shallow: '#4f8088' }, grade: { shadow: [0, 0, 0.01], light: [0.01, 0, 0], saturation: 1 },
    weather: horizon ? { sunny: { horizon } } : undefined,
});
const region = (id: string, l: Look): Region => ({ id, chain: [id], leaf: 'mixed', look: l });

describe('the look at the eye', () => {
    it('blends the haze, the grade and the weather of the regions about the eye by their weights', () => {
        const regions = [region('alps', look(22)), region('alps.insubric', look(11, '#d2d9db'))];
        const near = eyeLook(regions, new Map([[0, 0.75], [1, 0.25]]), 'sunny');
        expect(near.hazeKm).toBeCloseTo(0.75 * 22 + 0.25 * 11, 6);
        expect(near.weather.horizon[0]).toBeCloseTo(0.75 * linear('#b4bcc4')[0] + 0.25 * linear('#d2d9db')[0], 6);
        expect(eyeLook(regions, new Map([[1, 1]]), 'sunny').hazeKm).toBe(11);
    });
});
