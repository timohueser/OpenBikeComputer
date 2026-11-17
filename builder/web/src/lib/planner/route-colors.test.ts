import { describe, expect, it } from 'vitest';
import { ROUTE_COLORS, routeColor } from './route-colors';

describe('routeColor', () => {
    it('gives each relation ID one colour of the set, and uses the whole set', () => {
        expect(routeColor(3280)).toBe(routeColor('3280'));
        const used = new Set(Array.from({ length: 1000 }, (_, i) => routeColor(i + 1)));
        expect([...used].every(color => ROUTE_COLORS.includes(color))).toBe(true);
        expect(used.size).toBe(ROUTE_COLORS.length);
    });
});
