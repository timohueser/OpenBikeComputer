/**
 * The variant registry. Each lab lists its variants, the default first. `?lab=name:variant` selects
 * a variant; `?lab=a:x,b:y` or a repeated `lab` selects several.
 */
export const LABS: Record<string, readonly string[]> = {
    /**
     * The ground look: each pixel reads its texels through the region rules, or a per-page cache of the
     * ground parameters of each texel, made when the page arrives, which the pixel reads filtered.
     * Lighting, wetness and near detail stay per pixel in both.
     */
    ground: ['pixel', 'cache'],
};

export type Labs = Record<string, string>;

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
