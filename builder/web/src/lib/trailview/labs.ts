/**
 * The variant registry. Each lab lists its variants, the default first. `?lab=terrain:naive` selects
 * a variant; `?lab=terrain:naive,water:mesh` or a repeated `lab` selects several.
 */
export const LABS = {
    terrain: ['naive'],
} as const satisfies Record<string, readonly string[]>;

export type Labs = { -readonly [K in keyof typeof LABS]: (typeof LABS)[K][number] };

/** The variants of a query string. An unknown lab or variant throws, so a typo never shows the default. */
export function labsOf(search: string): Labs {
    const labs: Record<string, string> = Object.fromEntries(Object.entries(LABS).map(([name, variants]) => [name, variants[0]]));
    for (const entry of new URLSearchParams(search).getAll('lab').flatMap(value => value.split(','))) {
        const [name, variant] = entry.split(':');
        const variants: readonly string[] | undefined = (LABS as Record<string, readonly string[]>)[name];
        if (!variants?.includes(variant)) {
            const known = Object.entries(LABS).map(([lab, names]) => names.map(each => `${lab}:${each}`).join(', ')).join(', ');
            throw new Error(`The lab "${entry}" does not exist. The labs are ${known}.`);
        }
        labs[name] = variant;
    }
    return labs as Labs;
}
