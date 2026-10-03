import { describe, expect, it } from 'vitest';
import { addMonths, availableLayers, columnDate, dateColumn, monthColumns, type DataLayer } from './data-layer';

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

describe('calendar columns', () => {
    it('steps two days in 183 columns and a week in 52, with the rest of the year in the last', () => {
        expect([dateColumn('2026-01-02', 183), dateColumn('2026-01-03', 183), dateColumn('2026-12-31', 183)]).toEqual([0, 1, 182]);
        expect([dateColumn('2026-01-07', 52), dateColumn('2026-01-08', 52), dateColumn('2026-12-24', 52), dateColumn('2026-12-31', 52)]).toEqual([0, 1, 51, 51]);
        expect(dateColumn('2028-02-29', 52)).toBe(dateColumn('2028-02-28', 52));
        expect(columnDate(51, 2026, 52)).toBe('2026-12-24');
        expect(monthColumns(52).slice(0, 3)).toEqual([0, 5, 9]);
    });
});

describe('available layers', () => {
    const layer = (id: string) => ({ id }) as DataLayer;

    it('offers a layer only when the region has its archive, in list order', () => {
        const entries = [{ archive: 'snow', create: layer }, { archive: 'climate', create: layer }, { archive: 'climate', create: () => layer('wind') }] as const;
        expect(availableLayers(entries, { snow: '', climate: 'climate.json' }).map(l => l.id)).toEqual(['climate.json', 'wind']);
        expect(availableLayers(entries, { snow: 'snow.json', climate: '' }).map(l => l.id)).toEqual(['snow.json']);
    });
});
