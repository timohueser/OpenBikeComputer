import { describe, expect, it } from 'vitest';
import { airColour } from './light';

describe('the colour of the air', () => {
    it('leaves a high body white, turns it pale yellow below 15° and orange near the horizon', () => {
        const [high, yellow, orange] = [airColour(60), airColour(10), airColour(2)];
        expect(Math.min(...high)).toBeGreaterThan(0.95);
        expect(yellow[0]).toBe(1);
        expect(yellow[2]).toBeLessThan(0.5);
        expect(orange[1]).toBeLessThan(yellow[1] - 0.2);
    });
});
