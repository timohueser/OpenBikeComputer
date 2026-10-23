/**
 * The variant registry. Each lab lists its variants, the default first. `?lab=name:variant` selects
 * a variant; `?lab=a:x,b:y` or a repeated `lab` selects several.
 */
export const LABS: Record<string, readonly string[]> = {
    // The far trees: impostors, or the coarsest mesh.
    trees: ['impostor', 'mesh'],
    // Metres where the trees end and the woodland's shell begins.
    shell: ['4500', '2000'],
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
