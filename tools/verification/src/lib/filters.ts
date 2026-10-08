/** Sidebar filters of the requirements view, in chip order. The URL carries the key. */
export const FILTERS = { all: 'All', unassessed: 'Not assessed', partial: 'Partial', proposal: 'Has proposal', suggestion: 'Has suggestion' } as const;
export type Filter = keyof typeof FILTERS;
export const isFilter = (value: string | null): value is Filter => !!value && Object.hasOwn(FILTERS, value);
