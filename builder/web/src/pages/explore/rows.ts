/** The rows that the list of the Explore page draws at first, and that a scroll near their end adds. */
export const ROWS = 60;

/** The rows to draw for a new list: the first `ROWS`, and each row up to the selected one at index `selected` (-1 for
 * none), so the selected route always has its row. */
export const firstRows = (selected: number) => Math.max(ROWS, selected + 1);
