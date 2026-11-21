// Prototype #3765: throwaway switches for the phone lever measurement. They read the page's address; in a worker they keep the default.
const query = new URLSearchParams(globalThis.location?.search ?? '');

export const lever = (name: string, fallback: number): number => {
    const value = Number(query.get(name));
    return query.has(name) && Number.isFinite(value) ? value : fallback;
};

/** A count scaled by the `loads` lever, at least 1. */
export const loads = (count: number): number => Math.max(1, Math.round(count * lever('loads', 1)));
