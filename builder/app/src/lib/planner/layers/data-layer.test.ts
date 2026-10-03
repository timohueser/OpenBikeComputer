import { describe, expect, it } from 'vitest';
import { addMonths } from './data-layer';

describe('month steps', () => {
    it('keeps the day, or ends on the last day of a shorter month', () => {
        expect(addMonths('2026-06-15', 1)).toBe('2026-07-15');
        expect(addMonths('2026-01-31', 1)).toBe('2026-02-28');
        expect(addMonths('2028-01-31', 1)).toBe('2028-02-29');
        expect(addMonths('2026-03-31', -1)).toBe('2026-02-28');
        expect(addMonths('2026-12-31', 1)).toBe('2027-01-31');
        expect(addMonths('2026-01-15', -1)).toBe('2025-12-15');
    });
});
