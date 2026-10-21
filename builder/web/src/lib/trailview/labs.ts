/**
 * The variant registry. Each lab lists its variants, the default first. `?lab=terrain:rings` selects
 * a variant; `?lab=terrain:rings,pages:r16` or a repeated `lab` selects several.
 */
export const LABS = {
    /**
     * The LOD rule of the one quadtree: CDLOD by distance, or square rings about the eye as geometry
     * clipmaps select them. Both draw nodes with the same mesh and pages; a geometry clipmap with ring
     * textures and a toroidal update is not built.
     */
    terrain: ['cdlod', 'rings'],
    /** The micro-relief of the synthesized levels: none beyond the bicubic, or erosion gullies. */
    relief: ['bicubic', 'erosion'],
    /** The page heights: float32, or 16 bits with an offset and a scale per page. */
    pages: ['r32float', 'r16'],
    /**
     * The feature stamps and the water: stamps on every page at every level as it loads, or conditioning
     * baked into the stored levels, which the synthesized levels upsample, with no stamp of their own;
     * or neither stamps nor water, as L1 draws the terrain.
     */
    stamps: ['runtime', 'baked', 'none'],
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
