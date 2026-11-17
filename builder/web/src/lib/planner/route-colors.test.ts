import { describe, expect, it } from 'vitest';
import { ROUTE_COLORS, routeColor } from './route-colors';

describe('routeColor', () => {
    it('gives each relation ID the colour of the FNV-1a rule, and uses the whole set', () => {
        // A route keeps its colour in every view and every build, so these values must not change.
        expect(routeColor(3280)).toBe('#d63a00');
        expect(routeColor('-1234567')).toBe('#7a0a0a');
        const used = new Set(Array.from({ length: 1000 }, (_, i) => routeColor(i + 1)));
        expect([...used].every(color => ROUTE_COLORS.includes(color))).toBe(true);
        expect(used.size).toBe(ROUTE_COLORS.length);
    });
});
