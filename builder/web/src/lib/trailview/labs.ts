/**
 * The variant registry. Each lab lists its variants, the default first. `?lab=name:variant` selects
 * a variant; `?lab=a:x,b:y` or a repeated `lab` selects several.
 */
export const LABS: Record<string, readonly string[]> = {
    // The near sward: the prototype's grass tufts, or a sward of blades within 30 m.
    grass: ['tufts', 'blades'],
    // Temporary, for the owner: cheaper far trees that change the look. Where the trees end (m), where
    // the far trees start to thin (m), the texels of an impostor frame, where the coarsest mesh ends (m).
    reach: ['4500', '2500'],
    thin: ['600', '300'],
    atlas: ['32', '16'],
    meshes: ['700', '400'],
    // The share of the canopy's trees that stand, in per cent; the owner picks it by eye.
    density: ['70', '100', '90', '80', '60', '50'],
};

export type Labs = Record<string, string>;

/** The variants of a query string. An unknown lab or variant throws, so a typo never shows the default. */
export function labsOf(search: string): Labs {
    const labs: Labs = Object.fromEntries(Object.entries(LABS).map(([name, variants]) => [name, variants[0]]));
    for (const entry of new URLSearchParams(search).getAll('lab').flatMap(value => value.split(','))) {
        const [name, variant] = entry.split(':');
        const variants: readonly string[] | undefined = LABS[name];
        if (!variants?.includes(variant)) {
            const known = Object.entries(LABS).flatMap(([lab, names]) => names.map(each => `${lab}:${each}`));
            throw new Error(`The lab "${entry}" does not exist. ${known.length ? `The labs are ${known.join(', ')}.` : 'There are no labs.'}`);
        }
        labs[name] = variant;
    }
    return labs;
}
