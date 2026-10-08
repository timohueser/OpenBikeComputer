import { describe, expect, it } from 'vitest';
import { DRAWER_CLOSED, DRAWER_MIN, resizeDrawer } from './drawer';

describe('drawer resize', () => {
    it('crosses the gap with one key step in its direction', () => {
        expect(resizeDrawer(DRAWER_CLOSED, DRAWER_CLOSED + 16, true)).toBe(DRAWER_MIN);
        expect(resizeDrawer(DRAWER_MIN, DRAWER_MIN - 16, true)).toBe(DRAWER_CLOSED);
        // Above the minimum, a step stops at the minimum first; Home closes at once.
        expect(resizeDrawer(DRAWER_MIN + 10, DRAWER_MIN - 6, true)).toBe(DRAWER_MIN);
        expect(resizeDrawer(300, DRAWER_CLOSED, true)).toBe(DRAWER_CLOSED);
        expect(resizeDrawer(DRAWER_MIN, DRAWER_MIN + 16, true)).toBe(DRAWER_MIN + 16);
    });

    it('closes a drag past halfway and holds it at the minimum before', () => {
        const half = (DRAWER_MIN + DRAWER_CLOSED) / 2;
        expect(resizeDrawer(260, half + 1, false)).toBe(DRAWER_MIN);
        expect(resizeDrawer(260, half - 1, false)).toBe(DRAWER_CLOSED);
        expect(resizeDrawer(DRAWER_CLOSED, DRAWER_CLOSED + 16, false)).toBe(DRAWER_CLOSED);
        expect(resizeDrawer(DRAWER_CLOSED, 250, false)).toBe(250);
    });
});
