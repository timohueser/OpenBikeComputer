import { expect, it } from 'vitest';
import { firstRows, ROWS } from './rows';

it('draws the first rows of a new list, and each row up to the selected route', () => {
    expect(firstRows(-1)).toBe(ROWS);
    expect(firstRows(ROWS - 1)).toBe(ROWS);
    expect(firstRows(149)).toBe(150);
});
